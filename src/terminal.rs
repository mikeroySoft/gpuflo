//! Staged RAII terminal ownership for the interactive surface.
//!
//! Acquisition happens only after monitor preflight; each completed stage is
//! recorded so partial acquisition unwinds exactly what it enabled.
//! Restoration shows the cursor, disables raw mode, then leaves the
//! alternate screen — attempting every remaining step even when one fails —
//! and always precedes fatal diagnostics, monitor shutdown, persistence
//! flush, and joins. Non-interactive surfaces never instantiate this guard.

use std::io::Write;

/// The injectable terminal operation seam.
pub(crate) trait TerminalOps {
    fn enable_raw(&mut self) -> std::io::Result<()>;
    fn disable_raw(&mut self) -> std::io::Result<()>;
    fn enter_alternate_screen(&mut self) -> std::io::Result<()>;
    fn leave_alternate_screen(&mut self) -> std::io::Result<()>;
    fn hide_cursor(&mut self) -> std::io::Result<()>;
    fn show_cursor(&mut self) -> std::io::Result<()>;
}

/// Real crossterm-backed operations on stdout.
pub(crate) struct CrosstermOps;

impl TerminalOps for CrosstermOps {
    fn enable_raw(&mut self) -> std::io::Result<()> {
        crossterm::terminal::enable_raw_mode()
    }

    fn disable_raw(&mut self) -> std::io::Result<()> {
        crossterm::terminal::disable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> std::io::Result<()> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
    }

    fn leave_alternate_screen(&mut self) -> std::io::Result<()> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        let result = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        let _ = std::io::stdout().flush();
        result
    }
}

/// Staged guard over enabled terminal modes.
pub(crate) struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw: bool,
    alternate: bool,
    cursor_hidden: bool,
    restored: bool,
}

impl<O: TerminalOps> TerminalGuard<O> {
    /// Acquires raw mode, the alternate screen, and cursor hiding in order.
    /// A failure unwinds only the stages that actually completed.
    pub fn acquire(mut ops: O) -> std::io::Result<Self> {
        ops.enable_raw()?;
        if let Err(error) = ops.enter_alternate_screen() {
            let _ = ops.disable_raw();
            return Err(error);
        }
        if let Err(error) = ops.hide_cursor() {
            let _ = ops.leave_alternate_screen();
            let _ = ops.disable_raw();
            return Err(error);
        }
        Ok(Self {
            ops,
            raw: true,
            alternate: true,
            cursor_hidden: true,
            restored: false,
        })
    }

    /// Best-effort restoration: cursor, raw mode, then alternate screen.
    /// Every remaining step is attempted even when one fails; the first
    /// failure is reported. Idempotent.
    pub fn restore(&mut self) -> std::io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let mut first_error = None;
        if self.cursor_hidden {
            self.cursor_hidden = false;
            if let Err(error) = self.ops.show_cursor() {
                first_error.get_or_insert(error);
            }
        }
        if self.raw {
            self.raw = false;
            if let Err(error) = self.ops.disable_raw() {
                first_error.get_or_insert(error);
            }
        }
        if self.alternate {
            self.alternate = false;
            if let Err(error) = self.ops.leave_alternate_screen() {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        // Restoration failure must not panic during unwind.
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Default)]
    struct FakeState {
        calls: Vec<&'static str>,
        fail_on: Option<&'static str>,
    }

    #[derive(Clone)]
    struct FakeOps(Rc<RefCell<FakeState>>);

    impl FakeOps {
        fn new(fail_on: Option<&'static str>) -> Self {
            Self(Rc::new(RefCell::new(FakeState {
                calls: Vec::new(),
                fail_on,
            })))
        }

        fn record(&self, name: &'static str) -> std::io::Result<()> {
            let mut state = self.0.borrow_mut();
            state.calls.push(name);
            if state.fail_on == Some(name) {
                return Err(std::io::Error::other(name));
            }
            Ok(())
        }

        fn calls(&self) -> Vec<&'static str> {
            self.0.borrow().calls.clone()
        }
    }

    impl TerminalOps for FakeOps {
        fn enable_raw(&mut self) -> std::io::Result<()> {
            self.record("enable_raw")
        }

        fn disable_raw(&mut self) -> std::io::Result<()> {
            self.record("disable_raw")
        }

        fn enter_alternate_screen(&mut self) -> std::io::Result<()> {
            self.record("enter_alt")
        }

        fn leave_alternate_screen(&mut self) -> std::io::Result<()> {
            self.record("leave_alt")
        }

        fn hide_cursor(&mut self) -> std::io::Result<()> {
            self.record("hide_cursor")
        }

        fn show_cursor(&mut self) -> std::io::Result<()> {
            self.record("show_cursor")
        }
    }

    #[test]
    fn full_lifecycle_restores_in_cursor_raw_screen_order() {
        let ops = FakeOps::new(None);
        let guard = TerminalGuard::acquire(ops.clone()).unwrap();
        drop(guard);
        assert_eq!(
            ops.calls(),
            vec![
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "show_cursor",
                "disable_raw",
                "leave_alt",
            ]
        );
    }

    #[test]
    fn failed_first_stage_unwinds_nothing() {
        let ops = FakeOps::new(Some("enable_raw"));
        assert!(TerminalGuard::acquire(ops.clone()).is_err());
        assert_eq!(ops.calls(), vec!["enable_raw"]);
    }

    #[test]
    fn failed_second_stage_unwinds_only_raw_mode() {
        let ops = FakeOps::new(Some("enter_alt"));
        assert!(TerminalGuard::acquire(ops.clone()).is_err());
        assert_eq!(ops.calls(), vec!["enable_raw", "enter_alt", "disable_raw"]);
    }

    #[test]
    fn failed_third_stage_unwinds_screen_then_raw() {
        let ops = FakeOps::new(Some("hide_cursor"));
        assert!(TerminalGuard::acquire(ops.clone()).is_err());
        assert_eq!(
            ops.calls(),
            vec![
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "leave_alt",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn restore_attempts_every_step_even_when_one_fails() {
        let ops = FakeOps::new(Some("disable_raw"));
        let mut guard = TerminalGuard::acquire(ops.clone()).unwrap();
        assert!(guard.restore().is_err());
        assert_eq!(
            ops.calls(),
            vec![
                "enable_raw",
                "enter_alt",
                "hide_cursor",
                "show_cursor",
                "disable_raw",
                "leave_alt",
            ]
        );
        // Idempotent: dropping after an explicit restore adds nothing.
        drop(guard);
        assert_eq!(ops.calls().len(), 6);
    }
}
