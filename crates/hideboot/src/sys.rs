//! The one symbol the UEFI target lacks. Every `unsafe` in hideBoot is here.
//!
//! LLVM recognises a loop that counts UTF-16 units up to a terminator —
//! the length of a firmware string — and calls `wcslen` for it, which is a
//! C library function; on UEFI there is no C library, and compiler-builtins
//! does not provide this one.

#![allow(unsafe_code)]

/// The number of UTF-16 units before the terminator.
///
/// # Safety
///
/// `s` points to a sequence of `u16` that ends with a zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller promises a terminated sequence, so every unit up to
    // and including the terminator is readable. The volatile read keeps
    // LLVM from turning this loop back into a call to wcslen.
    while unsafe { core::ptr::read_volatile(s.add(n)) } != 0 {
        n += 1;
    }
    n
}
