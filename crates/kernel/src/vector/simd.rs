//! SIMD-accelerated distance kernels with run-time CPU dispatch.
//!
//! Public functions ([`l2_distance`], [`cosine_distance`], [`inner_product`])
//! pick the best implementation available for the host:
//!
//! * `x86_64` with AVX2 and FMA at runtime -> 8-wide AVX2 + FMA path.
//! * `aarch64` (NEON is mandatory in the AArch64 base ISA) -> 4-wide NEON path.
//! * Everything else -> scalar implementation from [`super::distance`].
//!
//! The AVX2 and NEON kernels live in `simd/avx2.rs` and `simd/neon.rs`; this
//! module keeps dispatch and the stable public API in one place.

use super::distance::{
    cosine_distance_scalar, inner_product_scalar, l2_distance_scalar, require_equal_len,
};

/// AVX2 kernels also execute FMA instructions. AVX2 without FMA stays on the
/// scalar path.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
fn host_avx2_fma() -> bool {
    std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[path = "simd/avx2.rs"]
mod avx2;
#[cfg(target_arch = "aarch64")]
#[path = "simd/neon.rs"]
mod neon;
#[cfg(test)]
#[path = "simd/tests.rs"]
mod tests;

/// Squared L2 distance with SIMD dispatch.
#[inline]
pub fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    require_equal_len(a, b);
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if host_avx2_fma() {
            // SAFETY: `host_avx2_fma` proved AVX2 and FMA. Lengths were asserted equal.
            unsafe {
                return avx2::l2_distance_avx2(a, b);
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: AArch64 base ISA includes NEON. Lengths were asserted equal.
        unsafe {
            return neon::l2_distance_neon(a, b);
        }
    }
    #[cfg_attr(target_arch = "aarch64", allow(unreachable_code))]
    l2_distance_scalar(a, b)
}

/// Cosine distance with SIMD dispatch.
#[inline]
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    require_equal_len(a, b);
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if host_avx2_fma() {
            // SAFETY: `host_avx2_fma` proved AVX2 and FMA. Lengths were asserted equal.
            unsafe {
                return avx2::cosine_distance_avx2(a, b);
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: AArch64 base ISA includes NEON. Lengths were asserted equal.
        unsafe {
            return neon::cosine_distance_neon(a, b);
        }
    }
    #[cfg_attr(target_arch = "aarch64", allow(unreachable_code))]
    cosine_distance_scalar(a, b)
}

/// Negative inner product with SIMD dispatch.
#[inline]
pub fn inner_product(a: &[f32], b: &[f32]) -> f32 {
    require_equal_len(a, b);
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if host_avx2_fma() {
            // SAFETY: `host_avx2_fma` proved AVX2 and FMA. Lengths were asserted equal.
            unsafe {
                return avx2::inner_product_avx2(a, b);
            }
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: AArch64 base ISA includes NEON. Lengths were asserted equal.
        unsafe {
            return neon::inner_product_neon(a, b);
        }
    }
    #[cfg_attr(target_arch = "aarch64", allow(unreachable_code))]
    inner_product_scalar(a, b)
}
