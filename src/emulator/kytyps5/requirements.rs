//! What KytyPS5 needs from this PC: Vulkan 1.3 with the extensions and features its
//! vulkanWindow.cpp (VulkanFindPhysicalDevice) rejects a device for.

use crate::pc::{Caps, Requirements};

/// Device extensions it refuses to start without. `a|b`: either (an extension promoted to KHR).
const EXTENSIONS: [&str; 8] = ["VK_KHR_swapchain", "VK_EXT_depth_clip_control", "VK_EXT_image_view_min_lod",
    "VK_KHR_push_descriptor", "VK_KHR_maintenance1", "VK_EXT_depth_clip_enable", "VK_EXT_color_write_enable",
    "VK_KHR_fragment_shader_barycentric"];

/// Extensions whose feature bits it checks.
const FEATURE_STRUCTS: &[(&str, u32)] = &[
    ("VK_KHR_fragment_shader_barycentric", 1000203000),
    ("VK_EXT_image_view_min_lod", 1000391000),
    ("VK_EXT_depth_clip_control", 1000355000),
    ("VK_EXT_depth_clip_enable", 1000102000),
    ("VK_EXT_color_write_enable", 1000381000),
];

pub fn requirements() -> Requirements {
    Requirements {
        needs_avx2: false,
        considers_only_passing: true,
        feature_structs: FEATURE_STRUCTS,
        check,
        // What the README asks for, a step up for PS5 games.
        min_vram_gb: 7.5,
        min_cores: 6,
        min_threads: 12,
        min_ram_gb: 14.5,
        advice_vram_gb: 8,
        advice_ram_gb: 16,
    }
}

/// Every check it rejects a device for, except the window surface's, which needs a window.
fn check(c: &Caps) -> bool {
    // VkPhysicalDeviceFeatures: robustBufferAccess, independentBlend, tessellationShader,
    // sampleRateShading, depthClamp, depthBiasClamp, fillModeNonSolid, depthBounds,
    // largePoints, multiViewport, samplerAnisotropy, fragmentStoresAndAtomics,
    // shaderImageGatherExtended, shaderStorageImageWriteWithoutFormat, shaderClipDistance,
    // shaderCullDistance.
    let core_ok = [0, 3, 5, 6, 11, 12, 13, 14, 16, 18, 19, 26, 28, 32, 37, 38].iter().all(|i| (c.core)(*i));
    // 1.1 storageBuffer16BitAccess; 1.2 samplerMirrorClampToEdge, storageBuffer8BitAccess,
    // shaderBufferInt64Atomics, shaderSampledImageArrayNonUniformIndexing,
    // timelineSemaphore, bufferDeviceAddress, shaderOutputViewportIndex, shaderOutputLayer;
    // 1.3 robustImageAccess, synchronization2, dynamicRendering.
    let versions_ok = (c.feature)("core 1.1", 0)
        && [0, 2, 5, 14, 37, 38, 44, 45].iter().all(|i| (c.feature)("core 1.2", *i))
        && [0, 9, 12].iter().all(|i| (c.feature)("core 1.3", *i));
    let extensions_ok = EXTENSIONS.iter().all(|e| (c.has)(e))
        && FEATURE_STRUCTS.iter().all(|(e, _)| (c.feature)(e, 0));
    let (depth, texture, storage) = (0x200, 0x1 | 0x8000, 0x2 | 0x8000);
    let format = |f: i32, need: u32| (c.format)(f, need);
    let formats_ok = [126, 130, 124].iter().all(|f| format(*f, depth)) // D32, D32S8, D16
        && [138, 43, 9, 16].iter().all(|f| format(*f, texture)) // BC3 sRGB, RGBA8 sRGB, R8, RG8
        && (format(43, storage) || format(37, storage)) // RGBA8 sRGB or UNORM
        && (format(50, storage) || format(44, storage)); // BGRA8 sRGB or UNORM
    c.vulkan >= (1, 3) && core_ok && versions_ok && extensions_ok && c.graphics_compute_queue && formats_ok && c.max_anisotropy >= 16.0
}
