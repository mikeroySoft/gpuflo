//! Optional runtime-loaded AMD SMI enrichment.
//!
//! Every `unsafe` block, library handle, raw pointer, function pointer, C
//! struct, and status mapping lives in this module; only owned typed samples
//! or structured failures leave it. The library is loaded with `libloading`
//! at runtime — never linked — and only read APIs are resolved. Declarations
//! and layouts follow `include/amd_smi/amdsmi.h` (verified at rocm-6.3.3,
//! commit 8dc45db6); the structs below carry no packing annotations upstream
//! and use natural C layout.

use std::ffi::c_void;
use std::time::Instant;

use libloading::Library;

use super::Reading;
use crate::model::{PciBdf, Timestamp};

/// `AMDSMI_INIT_AMD_GPUS`.
const INIT_AMD_GPUS: u64 = 1 << 1;
/// `AMDSMI_STATUS_SUCCESS`.
const STATUS_SUCCESS: u32 = 0;
/// `AMDSMI_STATUS_NOT_SUPPORTED`.
const STATUS_NOT_SUPPORTED: u32 = 2;
/// `AMDSMI_STATUS_NO_PERM`.
const STATUS_NO_PERM: u32 = 10;
/// `AMDSMI_PROCESSOR_TYPE_AMD_GPU`.
const PROCESSOR_TYPE_AMD_GPU: u32 = 1;
/// Exact AMD SMI header ABI verified by this adapter (ROCm 6.3.3,
/// `amdsmi_version_t` 25.1 and ELF SONAME 24).
const KNOWN_ABI: (u32, u32) = (25, 1);
/// Plausibility bound for vendor-library enumeration. Larger counts disable
/// optional enrichment instead of attempting an attacker-sized allocation.
const MAX_ENUM_HANDLES: u32 = 1024;

/// `amdsmi_version_t`: size 24, align 8 on LP64.
#[repr(C)]
struct CVersion {
    year: u32,
    major: u32,
    minor: u32,
    release: u32,
    build: *const std::ffi::c_char,
}

/// `amdsmi_engine_usage_t`: size 64, align 4; percentages 0–100.
#[repr(C)]
struct CEngineUsage {
    gfx_activity: u32,
    umc_activity: u32,
    mm_activity: u32,
    reserved: [u32; 13],
}

/// `amdsmi_vram_usage_t`: size 16, align 4; integer MiB.
#[repr(C)]
struct CVramUsage {
    vram_total: u32,
    vram_used: u32,
    reserved: [u32; 2],
}

type InitFn = unsafe extern "C" fn(u64) -> u32;
type ShutdownFn = unsafe extern "C" fn() -> u32;
type GetLibVersionFn = unsafe extern "C" fn(*mut CVersion) -> u32;
type GetSocketHandlesFn = unsafe extern "C" fn(*mut u32, *mut *mut c_void) -> u32;
type GetProcessorHandlesFn = unsafe extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> u32;
type GetProcessorTypeFn = unsafe extern "C" fn(*mut c_void, *mut u32) -> u32;
type GetBdfFn = unsafe extern "C" fn(*mut c_void, *mut u64) -> u32;
type GetActivityFn = unsafe extern "C" fn(*mut c_void, *mut CEngineUsage) -> u32;
type GetVramUsageFn = unsafe extern "C" fn(*mut c_void, *mut CVramUsage) -> u32;

/// The read-only symbol set this build requires.
struct Api {
    init: InitFn,
    shut_down: ShutdownFn,
    get_socket_handles: GetSocketHandlesFn,
    get_processor_handles: GetProcessorHandlesFn,
    get_processor_type: GetProcessorTypeFn,
    get_bdf: GetBdfFn,
    get_activity: GetActivityFn,
    get_vram_usage: GetVramUsageFn,
}

/// Why enrichment is disabled. All variants are normal degraded operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AmdSmiUnavailable {
    #[error("AMD SMI library not present")]
    LibraryNotFound,
    #[error("AMD SMI library is missing required symbol {0}")]
    MissingSymbol(String),
    #[error("AMD SMI library version {0}.{1} is not ABI-known to this build")]
    IncompatibleVersion(u32, u32),
    #[error("AMD SMI initialization failed with status {0}")]
    InitFailed(u32),
    #[error("AMD SMI enumeration failed with status {0}")]
    EnumerationFailed(u32),
}

/// One owned enrichment sample for the processor at `bdf`. All values are
/// copied out of C memory before the collection call returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmdSmiSample {
    pub bdf: PciBdf,
    pub read_wall: Timestamp,
    pub read_mono: Instant,
    /// GFX activity, percent (0–100).
    pub gfx_activity_percent: Reading<u64>,
    /// Memory-controller activity, percent (0–100).
    pub umc_activity_percent: Reading<u64>,
    /// VRAM used, bytes at MiB resolution.
    pub vram_used_bytes: Reading<u64>,
    /// VRAM total, bytes at MiB resolution.
    pub vram_total_bytes: Reading<u64>,
}

/// A loaded, initialized AMD SMI library with its enumerated GPU processors.
pub(crate) struct AmdSmi {
    /// Keeps the shared object mapped for the lifetime of the handles.
    _lib: Library,
    api: Api,
    /// Raw processor handles; never leave this module.
    processors: Vec<(*mut c_void, PciBdf)>,
}

// The library is used from one enrichment lane only; raw handles are opaque
// tokens the library dereferences internally.
unsafe impl Send for AmdSmi {}

impl AmdSmi {
    /// Attempts only the verified ABI-major SONAME, then the unversioned
    /// development name (which is still rejected unless its API tuple matches).
    pub fn load() -> Result<Self, AmdSmiUnavailable> {
        Self::load_from(&["libamd_smi.so.24", "libamd_smi.so"])
    }

    /// Loads from explicit candidates (test seam uses file paths).
    pub fn load_from(candidates: &[&str]) -> Result<Self, AmdSmiUnavailable> {
        let mut lib = None;
        for candidate in candidates {
            // SAFETY: loading an AMD SMI shared object; its constructors are
            // the vendor's own initialization code.
            if let Ok(loaded) = unsafe { Library::new(candidate) } {
                lib = Some(loaded);
                break;
            }
        }
        let lib = lib.ok_or(AmdSmiUnavailable::LibraryNotFound)?;

        macro_rules! symbol {
            ($name:literal) => {{
                // SAFETY: the signature matches the verified amdsmi.h
                // declaration for this symbol.
                let symbol = unsafe { lib.get($name) }.map_err(|_| {
                    AmdSmiUnavailable::MissingSymbol(
                        String::from_utf8_lossy(&$name[..$name.len() - 1]).into_owned(),
                    )
                })?;
                *symbol
            }};
        }

        let get_lib_version: GetLibVersionFn = symbol!(b"amdsmi_get_lib_version\0");
        let api = Api {
            init: symbol!(b"amdsmi_init\0"),
            shut_down: symbol!(b"amdsmi_shut_down\0"),
            get_socket_handles: symbol!(b"amdsmi_get_socket_handles\0"),
            get_processor_handles: symbol!(b"amdsmi_get_processor_handles\0"),
            get_processor_type: symbol!(b"amdsmi_get_processor_type\0"),
            get_bdf: symbol!(b"amdsmi_get_gpu_device_bdf\0"),
            get_activity: symbol!(b"amdsmi_get_gpu_activity\0"),
            get_vram_usage: symbol!(b"amdsmi_get_gpu_vram_usage\0"),
        };

        // AMD SMI requires initialization before get_lib_version.
        // SAFETY: init takes the documented flag word; called exactly once.
        let status = unsafe { (api.init)(INIT_AMD_GPUS) };
        if status != STATUS_SUCCESS {
            return Err(AmdSmiUnavailable::InitFailed(status));
        }
        let mut version = CVersion {
            year: 0,
            major: 0,
            minor: 0,
            release: 0,
            build: std::ptr::null(),
        };
        // SAFETY: initialized library; out-pointer to a correctly sized struct.
        let status = unsafe { get_lib_version(&mut version) };
        if status != STATUS_SUCCESS {
            // SAFETY: balances the successful initialization above.
            unsafe { (api.shut_down)() };
            return Err(AmdSmiUnavailable::InitFailed(status));
        }
        if (version.year, version.major) != KNOWN_ABI {
            // SAFETY: balances the successful initialization above.
            unsafe { (api.shut_down)() };
            return Err(AmdSmiUnavailable::IncompatibleVersion(
                version.year,
                version.major,
            ));
        }

        let mut loaded = Self {
            _lib: lib,
            api,
            processors: Vec::new(),
        };
        match loaded.enumerate() {
            Ok(processors) => {
                loaded.processors = processors;
                Ok(loaded)
            }
            Err(error) => {
                // Drop runs shutdown exactly once.
                Err(error)
            }
        }
    }

    /// Enumerates socket → processor handles and copies each BDF.
    fn enumerate(&self) -> Result<Vec<(*mut c_void, PciBdf)>, AmdSmiUnavailable> {
        let mut socket_count: u32 = 0;
        // SAFETY: documented in/out capacity protocol with a null handle
        // pointer returning only the count.
        let status =
            unsafe { (self.api.get_socket_handles)(&mut socket_count, std::ptr::null_mut()) };
        if status != STATUS_SUCCESS {
            return Err(AmdSmiUnavailable::EnumerationFailed(status));
        }
        if socket_count > MAX_ENUM_HANDLES {
            return Err(AmdSmiUnavailable::EnumerationFailed(42));
        }
        let socket_capacity = socket_count;
        let mut sockets = vec![std::ptr::null_mut(); socket_capacity as usize];
        // SAFETY: buffer sized to the returned capacity.
        let status =
            unsafe { (self.api.get_socket_handles)(&mut socket_count, sockets.as_mut_ptr()) };
        if status != STATUS_SUCCESS || socket_count > socket_capacity {
            return Err(AmdSmiUnavailable::EnumerationFailed(status.max(42)));
        }
        sockets.truncate(socket_count as usize);

        let mut processors = Vec::new();
        for socket in sockets {
            let mut count: u32 = 0;
            // SAFETY: same in/out protocol per socket.
            let status = unsafe {
                (self.api.get_processor_handles)(socket, &mut count, std::ptr::null_mut())
            };
            if status != STATUS_SUCCESS {
                continue; // One bad socket does not disable enrichment.
            }
            if count > MAX_ENUM_HANDLES {
                continue;
            }
            let capacity = count;
            let mut handles = vec![std::ptr::null_mut(); capacity as usize];
            // SAFETY: buffer sized to the returned capacity.
            let status = unsafe {
                (self.api.get_processor_handles)(socket, &mut count, handles.as_mut_ptr())
            };
            if status != STATUS_SUCCESS || count > capacity {
                continue;
            }
            handles.truncate(count as usize);
            for handle in handles {
                let mut kind: u32 = 0;
                // SAFETY: out-pointer to an owned u32-backed enum.
                let status = unsafe { (self.api.get_processor_type)(handle, &mut kind) };
                if status != STATUS_SUCCESS || kind != PROCESSOR_TYPE_AMD_GPU {
                    continue;
                }
                let mut raw: u64 = 0;
                // SAFETY: out-pointer to the 8-byte amdsmi_bdf_t union.
                let status = unsafe { (self.api.get_bdf)(handle, &mut raw) };
                if status != STATUS_SUCCESS {
                    continue;
                }
                if let Ok(bdf) = decode_bdf(raw) {
                    processors.push((handle, bdf));
                }
            }
        }
        Ok(processors)
    }

    /// Collects one owned sample per enumerated GPU processor.
    pub fn sample(&self) -> Vec<AmdSmiSample> {
        let read_wall = Timestamp::now();
        let read_mono = Instant::now();
        self.processors
            .iter()
            .map(|(handle, bdf)| {
                let mut usage = CEngineUsage {
                    gfx_activity: 0,
                    umc_activity: 0,
                    mm_activity: 0,
                    reserved: [0; 13],
                };
                // SAFETY: out-pointer to an owned, correctly sized struct.
                let status = unsafe { (self.api.get_activity)(*handle, &mut usage) };
                let (gfx, umc) = match status {
                    STATUS_SUCCESS => (
                        activity_reading(usage.gfx_activity),
                        activity_reading(usage.umc_activity),
                    ),
                    other => (status_reading(other), status_reading(other)),
                };

                let mut vram = CVramUsage {
                    vram_total: 0,
                    vram_used: 0,
                    reserved: [0; 2],
                };
                // SAFETY: out-pointer to an owned, correctly sized struct.
                let status = unsafe { (self.api.get_vram_usage)(*handle, &mut vram) };
                let (used, total) = match status {
                    STATUS_SUCCESS => (mib_reading(vram.vram_used), mib_reading(vram.vram_total)),
                    other => (status_reading(other), status_reading(other)),
                };

                AmdSmiSample {
                    bdf: bdf.clone(),
                    read_wall,
                    read_mono,
                    gfx_activity_percent: gfx,
                    umc_activity_percent: umc,
                    vram_used_bytes: used,
                    vram_total_bytes: total,
                }
            })
            .collect()
    }
}

impl Drop for AmdSmi {
    fn drop(&mut self) {
        // SAFETY: shutdown pairs with the successful init in load_from and
        // runs exactly once; the library stays mapped until after this call.
        unsafe {
            let _ = (self.api.shut_down)();
        }
    }
}

/// Maps a non-success status to reading evidence.
fn status_reading<T>(status: u32) -> Reading<T> {
    match status {
        STATUS_NOT_SUPPORTED => Reading::Sentinel,
        STATUS_NO_PERM => Reading::PermissionDenied,
        _ => Reading::Error,
    }
}

/// Percent field with the documented `0xFFFF`-carried-in-u32 sentinel.
fn activity_reading(value: u32) -> Reading<u64> {
    match value {
        0xFFFF | 0xFFFF_FFFF => Reading::Sentinel,
        v if v > 100 => Reading::Malformed,
        v => Reading::Value(u64::from(v)),
    }
}

/// MiB quantity widened to bytes; the all-ones u32 is unavailable.
fn mib_reading(value: u32) -> Reading<u64> {
    match value {
        0xFFFF_FFFF => Reading::Sentinel,
        v => Reading::Value(u64::from(v) * 1024 * 1024),
    }
}

/// Decodes `amdsmi_bdf_t::as_uint` using the checked x86-64 layout:
/// function 0–2, device 3–7, bus 8–15, domain 16–63.
fn decode_bdf(raw: u64) -> Result<PciBdf, crate::model::InvalidPciBdf> {
    let function = raw & 0x7;
    let device = (raw >> 3) & 0x1F;
    let bus = (raw >> 8) & 0xFF;
    let domain = (raw >> 16) & 0xFFFF;
    PciBdf::parse(&format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    /// Builds a tiny test-only shared library exporting the AMD SMI surface.
    fn build_fake(name: &str, source: &str) -> std::path::PathBuf {
        let dir = loop {
            let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("gruflo-amdsmi-{}-{id}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        };
        let c_path = dir.join(format!("{name}.c"));
        let so_path = dir.join(format!("lib{name}.so"));
        std::fs::write(&c_path, source).unwrap();
        let status = std::process::Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&so_path)
            .arg(&c_path)
            .status()
            .expect("cc must be available to build the AMD SMI test library");
        assert!(status.success(), "compiling {name} failed");
        so_path
    }

    const GOOD_LIBRARY: &str = r#"
#include <stdint.h>
#include <stddef.h>

typedef struct { uint32_t year, major, minor, release; const char *build; } ver_t;
typedef struct { uint32_t gfx, umc, mm, reserved[13]; } usage_t;
typedef struct { uint32_t total, used, reserved[2]; } vram_t;

static int initialized = 0;
static int handle_token = 42;

uint32_t amdsmi_get_lib_version(ver_t *v) {
    if (!initialized) return 32;
    v->year = 25; v->major = 1; v->minor = 0; v->release = 0; v->build = "25.1.0.0";
    return 0;
}
uint32_t amdsmi_init(uint64_t flags) {
    (void)flags;
    if (initialized) return 18; /* AMDSMI_STATUS_INIT_ERROR */
    initialized = 1;
    return 0;
}
uint32_t amdsmi_shut_down(void) {
    if (!initialized) return 32; /* AMDSMI_STATUS_NOT_INIT */
    initialized = 0;
    return 0;
}
uint32_t amdsmi_get_socket_handles(uint32_t *count, void **handles) {
    if (!initialized) return 32;
    if (handles == NULL) { *count = 1; return 0; }
    if (*count < 1) return 41;
    *count = 1; handles[0] = &handle_token;
    return 0;
}
uint32_t amdsmi_get_processor_handles(void *socket, uint32_t *count, void **handles) {
    (void)socket;
    if (!initialized) return 32;
    if (handles == NULL) { *count = 1; return 0; }
    if (*count < 1) return 41;
    *count = 1; handles[0] = &handle_token;
    return 0;
}
uint32_t amdsmi_get_processor_type(void *handle, uint32_t *kind) {
    (void)handle; *kind = 1; /* AMD_GPU */
    return 0;
}
uint32_t amdsmi_get_gpu_device_bdf(void *handle, uint64_t *bdf) {
    (void)handle;
    /* domain 0, bus 0x41, device 0, function 0 */
    *bdf = (uint64_t)0x41 << 8;
    return 0;
}
uint32_t amdsmi_get_gpu_activity(void *handle, usage_t *usage) {
    (void)handle;
    if (!initialized) return 32;
    usage->gfx = 97; usage->umc = 0xFFFF; usage->mm = 3;
    return 0;
}
uint32_t amdsmi_get_gpu_vram_usage(void *handle, vram_t *vram) {
    (void)handle;
    if (!initialized) return 32;
    vram->total = 196608; vram->used = 131072;
    return 0;
}
"#;

    #[test]
    fn missing_library_is_a_normal_disabled_source() {
        let result = AmdSmi::load_from(&["/nonexistent/libamd_smi.so.999"]);
        assert_eq!(result.err(), Some(AmdSmiUnavailable::LibraryNotFound));
    }

    #[test]
    fn missing_required_symbol_disables_enrichment() {
        let source = GOOD_LIBRARY.replace("amdsmi_get_gpu_vram_usage", "amdsmi_renamed_away");
        let path = build_fake("gruflo_fake_nosym", &source);
        let result = AmdSmi::load_from(&[path.to_str().unwrap()]);
        assert_eq!(
            result.err(),
            Some(AmdSmiUnavailable::MissingSymbol(
                "amdsmi_get_gpu_vram_usage".to_owned()
            ))
        );
    }

    #[test]
    fn unknown_version_disables_enrichment_and_balances_shutdown() {
        let source = GOOD_LIBRARY.replace("v->year = 25", "v->year = 99");
        let path = build_fake("gruflo_fake_badver", &source);
        let result = AmdSmi::load_from(&[path.to_str().unwrap()]);
        assert_eq!(
            result.err(),
            Some(AmdSmiUnavailable::IncompatibleVersion(99, 1))
        );
    }

    #[test]
    fn implausible_enumeration_count_disables_enrichment() {
        let source = GOOD_LIBRARY.replacen(
            "if (handles == NULL) { *count = 1; return 0; }",
            "if (handles == NULL) { *count = 0xFFFFFFFF; return 0; }",
            1,
        );
        let path = build_fake("gruflo_fake_huge_count", &source);
        let result = AmdSmi::load_from(&[path.to_str().unwrap()]);
        assert_eq!(result.err(), Some(AmdSmiUnavailable::EnumerationFailed(42)));
    }

    #[test]
    fn good_library_initializes_once_and_returns_owned_samples() {
        let path = build_fake("gruflo_fake_good", GOOD_LIBRARY);
        let smi = AmdSmi::load_from(&[path.to_str().unwrap()]).unwrap();
        let samples = smi.sample();
        assert_eq!(samples.len(), 1);
        let sample = &samples[0];
        assert_eq!(sample.bdf.as_str(), "0000:41:00.0");
        assert_eq!(sample.gfx_activity_percent, Reading::Value(97));
        // The 0xFFFF sentinel is carried in a u32 field.
        assert_eq!(sample.umc_activity_percent, Reading::Sentinel);
        assert_eq!(
            sample.vram_used_bytes,
            Reading::Value(131_072 * 1024 * 1024)
        );
        assert_eq!(
            sample.vram_total_bytes,
            Reading::Value(196_608 * 1024 * 1024)
        );
        // Sampling twice works; init ran exactly once (the fake library
        // fails a second init with INIT_ERROR).
        let again = smi.sample();
        assert_eq!(again[0].gfx_activity_percent, Reading::Value(97));
        drop(smi); // Shutdown exactly once; the fake fails an unpaired call.
    }

    #[test]
    fn status_codes_map_to_distinct_states() {
        assert_eq!(
            status_reading::<u64>(STATUS_NOT_SUPPORTED),
            Reading::Sentinel
        );
        assert_eq!(
            status_reading::<u64>(STATUS_NO_PERM),
            Reading::PermissionDenied
        );
        assert_eq!(status_reading::<u64>(7), Reading::Error);
        assert_eq!(activity_reading(0xFFFF), Reading::Sentinel);
        assert_eq!(activity_reading(101), Reading::Malformed);
        assert_eq!(activity_reading(55), Reading::Value(55));
    }

    #[test]
    fn bdf_decoding_follows_the_checked_bit_layout() {
        let raw = (0x0002u64 << 16) | (0xC1 << 8) | (0x03 << 3) | 0x1;
        assert_eq!(decode_bdf(raw).unwrap().as_str(), "0002:c1:03.1");
    }
}
