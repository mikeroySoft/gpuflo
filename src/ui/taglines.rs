//! Session-stable launch taglines for the full mode surface.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::{SystemTime, UNIX_EPOCH};

const ALL: [&str; 100] = [
    "Power in, tokens out.",
    "Pixels dream in parallel.",
    "The silicon is having a lovely day.",
    "Tiny sparks, big ideas.",
    "Your GPU believes in you.",
    "Compute boldly, rest gently.",
    "Every cycle carries possibility.",
    "More math, more magic.",
    "Good vibes at full bandwidth.",
    "The future is locally rendered.",
    "Parallel paths, shared purpose.",
    "Keep calm and multiply matrices.",
    "Let the tensors find their shape.",
    "Bright ideas love fast memory.",
    "Crunch kindly.",
    "Local compute, cosmic ambition.",
    "Every watt has a story.",
    "Friendly silicon, serious work.",
    "Ideas flow better in parallel.",
    "The kernel woke up optimistic.",
    "Smooth clocks, clear thoughts.",
    "Your models are in good hands.",
    "Make something wonderfully useful.",
    "Powered by electrons and curiosity.",
    "Big bandwidth, bigger imagination.",
    "Warm cores, cool possibilities.",
    "Threads together, strong together.",
    "Keep your vectors pointed forward.",
    "A little compute goes a long way.",
    "Math is just organized wonder.",
    "Today is a good day to accelerate.",
    "Small steps, massively parallel.",
    "The buses are humming happily.",
    "All systems glow.",
    "Good work deserves good telemetry.",
    "The matrix is on your side.",
    "Stay curious at every clock speed.",
    "Memory full of bright ideas.",
    "Let the wavefronts wander wisely.",
    "Progress looks great in real time.",
    "Curious minds compile beautiful things.",
    "Delightfully overqualified arithmetic.",
    "Every thread brings something special.",
    "A bright future fits in VRAM.",
    "Fast thoughts, gentle temperatures.",
    "The bits are feeling adventurous.",
    "Keep building the improbable.",
    "Thoughtful code, cheerful hardware.",
    "Local power, limitless creativity.",
    "The GPU brought its best graphs.",
    "One more cycle toward something great.",
    "Bandwidth is a team sport.",
    "Many cores, one excellent plan.",
    "Your next idea is already warming up.",
    "Calm telemetry, bold experiments.",
    "The fans are your tiny wind choir.",
    "Make room for marvelous results.",
    "Fresh samples, fresh possibilities.",
    "The future enjoys low latency.",
    "Every register holds a little hope.",
    "Joy, now with hardware acceleration.",
    "Neat numbers, nicer days.",
    "Speed with a sunny disposition.",
    "Data goes in, insight comes out.",
    "Be excellent to your accelerators.",
    "Your workload found its happy place.",
    "Parallelism makes room for wonder.",
    "The machine is rooting for you.",
    "Plenty of compute to go around.",
    "Numbers can dance too.",
    "Today, the tensors align.",
    "The clocks are keeping good time.",
    "A good graph tells a hopeful story.",
    "Tiny transistors, tremendous teamwork.",
    "Strong signals, smooth sailing.",
    "The GPU packed extra enthusiasm.",
    "Let every core contribute.",
    "Memory is making happy connections.",
    "Useful things are taking shape.",
    "The workload is finding its rhythm.",
    "Create locally, imagine globally.",
    "The shortest path is often parallel.",
    "One machine, countless possibilities.",
    "Kind code runs beautifully.",
    "The graph is trending toward awesome.",
    "Bright minds deserve bright pixels.",
    "Compute with confidence.",
    "Great ideas scale gracefully.",
    "The machine room feels inspiring today.",
    "Every sample is a small discovery.",
    "Stay warm, stay curious.",
    "A cheerful cluster starts with one GPU.",
    "Math loves good company.",
    "Let your ideas use every lane.",
    "Good things come to those who compute.",
    "Possibility is highly parallel.",
    "Your silicon has excellent manners.",
    "Make today beautifully efficient.",
    "The next breakthrough is compiling.",
    "Powered by curiosity and sensible defaults.",
];

/// Selects one tagline when session state is created. The borrowed value stays
/// unchanged for the lifetime of that state.
pub(super) fn select() -> &'static str {
    let launch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let index = RandomState::new().hash_one((std::process::id(), launch)) as usize % ALL.len();
    ALL[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_has_one_hundred_unique_terminal_safe_taglines() {
        assert_eq!(ALL.len(), 100);
        for (index, tagline) in ALL.iter().enumerate() {
            assert!(!tagline.is_empty(), "tagline {index} is empty");
            assert!(
                tagline.chars().all(|character| !character.is_control()),
                "tagline {index} contains a control character"
            );
            assert!(
                tagline.chars().count() <= 60,
                "tagline {index} is too wide for the mode surface"
            );
            assert_eq!(
                ALL.iter().filter(|candidate| *candidate == tagline).count(),
                1,
                "tagline {index} is duplicated"
            );
        }
    }
}
