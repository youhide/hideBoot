//! Every `unsafe` in hideBoot is here: the one symbol the UEFI target
//! lacks, and the graphics output, opened without taking it from the
//! firmware's console.
//!
//! LLVM recognises a loop that counts UTF-16 units up to a terminator —
//! the length of a firmware string — and calls `wcslen` for it, which is a
//! C library function; on UEFI there is no C library, and compiler-builtins
//! does not provide this one.

#![allow(unsafe_code)]

extern crate alloc;

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

/// The graphics output, shared: opened for its interface only, so the
/// firmware's console keeps it, and stays usable if hideBoot returns to the
/// firmware. With it, the display's EDID, where the firmware has one: the
/// active one, else the one it discovered. `None` where there is no
/// graphics output — a serial console.
pub fn graphics_output() -> Option<(
    uefi::boot::ScopedProtocol<uefi::proto::console::gop::GraphicsOutput>,
    Option<alloc::vec::Vec<u8>>,
)> {
    use uefi::boot;
    use uefi::proto::console::gop::GraphicsOutput;
    let handle = boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
    let gop = shared::<GraphicsOutput>(handle)?;
    let edid = edid::<EdidActive>(handle).or_else(|| edid::<EdidDiscovered>(handle));
    Some((gop, edid))
}

/// A protocol on `handle`, opened for its interface only.
fn shared<P: uefi::proto::ProtocolPointer + ?Sized>(
    handle: uefi::Handle,
) -> Option<uefi::boot::ScopedProtocol<P>> {
    use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
    // SAFETY: GetProtocol opens the interface without the exclusive access
    // that would disconnect the console from the graphics output. hideBoot
    // reads EDID through it, and draws only through the graphics output's
    // own Blt and SetMode, while the menu is up; the console prints nothing
    // over it then but the text menu, drawn before each picture.
    unsafe {
        boot::open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
    .ok()
}

/// EFI_EDID_ACTIVE_PROTOCOL and EFI_EDID_DISCOVERED_PROTOCOL, from the UEFI
/// specification, 12.9: a size and a pointer to the EDID bytes.
#[repr(C)]
struct EdidData {
    size: u32,
    edid: *const u8,
}

#[repr(transparent)]
#[uefi::proto::unsafe_protocol("bd8c1056-9f36-44ec-92a8-a6337f817986")]
struct EdidActive(EdidData);

#[repr(transparent)]
#[uefi::proto::unsafe_protocol("1c0c34f6-d380-41fa-a049-8ad06c1a66aa")]
struct EdidDiscovered(EdidData);

trait Edid {
    fn data(&self) -> &EdidData;
}

impl Edid for EdidActive {
    fn data(&self) -> &EdidData {
        &self.0
    }
}

impl Edid for EdidDiscovered {
    fn data(&self) -> &EdidData {
        &self.0
    }
}

/// The EDID bytes, copied out of the firmware's buffer.
fn edid<P: Edid + uefi::proto::ProtocolPointer>(handle: uefi::Handle) -> Option<alloc::vec::Vec<u8>> {
    let protocol = shared::<P>(handle)?;
    let data = protocol.data();
    let size = usize::try_from(data.size).ok()?;
    if data.edid.is_null() || size == 0 {
        return None;
    }
    // SAFETY: the specification makes `edid` point to `size` bytes, owned
    // by the firmware for as long as the protocol is installed, which it is
    // while `protocol` is open; they are copied before it closes.
    let bytes = unsafe { core::slice::from_raw_parts(data.edid, size) };
    Some(bytes.to_vec())
}
