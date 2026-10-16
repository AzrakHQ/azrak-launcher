//! What shadPS4 needs from this PC: AVX2, and the Vulkan extensions it asserts in its
//! vk_instance.cpp plus robustness2's features.

use crate::pc::{Caps, Requirements};

/// Device extensions it asserts. `a|b`: either (an extension promoted to KHR).
const EXTENSIONS: [&str; 4] = ["VK_KHR_swapchain", "VK_KHR_push_descriptor",
    "VK_EXT_vertex_attribute_divisor|VK_KHR_vertex_attribute_divisor", "VK_EXT_robustness2|VK_KHR_robustness2"];

const ROBUSTNESS2: &str = "VK_EXT_robustness2|VK_KHR_robustness2";

pub fn requirements() -> Requirements {
    Requirements {
        needs_avx2: true,
        considers_only_passing: false,
        feature_structs: &[(ROBUSTNESS2, 1000286000)],
        check,
        min_vram_gb: 3.5,
        min_cores: 4,
        min_threads: 8,
        min_ram_gb: 7.0,
        advice_vram_gb: 4,
        advice_ram_gb: 8,
    }
}

fn check(c: &Caps) -> bool {
    EXTENSIONS.iter().all(|e| (c.has)(e)) && (0..3).all(|i| (c.feature)(ROBUSTNESS2, i))
}
