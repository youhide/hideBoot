//! The firmware side: files, variables, keys, and loading an image.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;

use hideboot_core::{Entry, order};
use uefi::boot::{self, LoadImageSource};
use uefi::data_types::Align;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{CString16, Status, guid, println};

const LINUX_DIR: &str = "\\EFI\\Linux";
/// Recovery images: in the menu, and booted by themselves only when no
/// entry in `\EFI\Linux` will start. Never counted, never the default.
const RECOVERY_DIR: &str = "\\EFI\\Recovery";
/// systemd's vendor GUID for the boot loader interface, which the OS reads.
const LOADER_VENDOR: VariableVendor = VariableVendor(guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"));

#[uefi::entry]
fn main() -> Status {
    match run() {
        Ok(()) => Status::SUCCESS,
        Err(error) => {
            println!("hideBoot: {error}");
            println!("hideBoot: nothing could be booted; returning to the firmware in 30 seconds");
            boot::stall(Duration::from_secs(30));
            Status::LOAD_ERROR
        }
    }
}

#[derive(Debug)]
enum Error {
    Firmware(&'static str, Status),
    NoEntries,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Firmware(what, status) => write!(f, "{what}: {status:?}"),
            Error::NoEntries => write!(f, "no kernel image in {LINUX_DIR}"),
        }
    }
}

fn firmware<T: fmt::Debug>(what: &'static str) -> impl FnOnce(uefi::Error<T>) -> Error {
    move |error| Error::Firmware(what, error.status())
}

fn run() -> Result<(), Error> {
    let mut volume = boot::get_image_file_system(boot::image_handle())
        .map_err(firmware("opening the ESP"))?
        .open_volume()
        .map_err(firmware("opening the ESP"))?;
    let mut entries = list(&mut volume, LINUX_DIR)?;
    order(&mut entries);
    let mut recovery = list(&mut volume, RECOVERY_DIR)?;
    order(&mut recovery);
    let candidates: Vec<(&str, Entry)> = entries
        .into_iter()
        .map(|e| (LINUX_DIR, e))
        .chain(recovery.into_iter().map(|e| (RECOVERY_DIR, e)))
        .collect();
    if candidates.is_empty() {
        return Err(Error::NoEntries);
    }
    set_variable("LoaderInfo", "hideBoot 0.1");

    // The chosen entry first, then the rest in order: an image the firmware
    // refuses — a bad signature, a corrupt file — is passed over, as a
    // boot that failed, and recovery comes last.
    let first = if key_held() { menu(&candidates) } else { 0 };
    let order = core::iter::once(first).chain((0..candidates.len()).filter(|&i| i != first));
    for index in order {
        let Some((dir, entry)) = candidates.get(index) else {
            continue;
        };
        match attempt(&mut volume, dir, entry) {
            Ok(()) => return Ok(()),
            Err(error) => println!("hideBoot: {}: {error}", entry.file_name()),
        }
    }
    Err(Error::Firmware(
        "every kernel image failed",
        Status::LOAD_ERROR,
    ))
}

/// The entries in `dir`; none when there is no such directory.
fn list(volume: &mut Directory, dir: &str) -> Result<Vec<Entry>, Error> {
    let path = cstring(dir)?;
    let mut directory = match volume.open(&path, FileMode::Read, FileAttribute::empty()) {
        Ok(handle) => match handle.into_directory() {
            Some(directory) => directory,
            None => return Ok(Vec::new()),
        },
        Err(error) if error.status() == Status::NOT_FOUND => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::Firmware(
                "opening a directory on the ESP",
                error.status(),
            ));
        }
    };
    let mut entries = Vec::new();
    while let Some(info) = directory
        .read_entry_boxed()
        .map_err(firmware("reading a directory on the ESP"))?
    {
        if info.is_directory() {
            continue;
        }
        if let Some(entry) = Entry::parse(&info.file_name().to_string()) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Counts the attempt, tells the OS what it booted, and starts the image.
/// Returns only when the image could not be started.
fn attempt(volume: &mut Directory, dir: &str, entry: &Entry) -> Result<(), Error> {
    let name = entry.file_name();
    let image = read(volume, dir, &name)?;
    // The rename before the start: a boot that hangs or resets has used
    // the attempt, and the firmware forgets nothing on disk.
    let booted = match entry.after_attempt().filter(|_| dir == LINUX_DIR) {
        Some(next) => {
            rename(volume, dir, &name, &next.file_name())?;
            next.file_name()
        }
        None => name,
    };
    set_variable("LoaderBootCountPath", &format!("{dir}\\{booted}"));
    set_variable("LoaderEntrySelected", &booted);

    let handle = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromBuffer {
            buffer: &image,
            file_path: None,
        },
    )
    .map_err(firmware("loading the image"))?;
    drop(image);
    boot::start_image(handle).map_err(firmware("starting the image"))?;
    // A kernel image that returns did not boot.
    Err(Error::Firmware("the image returned", Status::ABORTED))
}

fn read(volume: &mut Directory, dir: &str, name: &str) -> Result<Vec<u8>, Error> {
    let mut file = open_file(volume, dir, name, FileMode::Read)?;
    let info = file
        .get_boxed_info::<FileInfo>()
        .map_err(firmware("reading the image's size"))?;
    let size = usize::try_from(info.file_size())
        .map_err(|_| Error::Firmware("the image is too large", Status::BAD_BUFFER_SIZE))?;
    let mut data = vec![0u8; size];
    let mut done = 0;
    while done < size {
        let rest = data.get_mut(done..).unwrap_or_default();
        let n = file.read(rest).map_err(firmware("reading the image"))?;
        if n == 0 {
            return Err(Error::Firmware("the image ends early", Status::END_OF_FILE));
        }
        done += n;
    }
    Ok(data)
}

/// Renames in place, through the directory entry, as systemd-boot does: no
/// copy, and nothing in between for a power cut to find.
fn rename(volume: &mut Directory, dir: &str, from: &str, to: &str) -> Result<(), Error> {
    let mut file = open_file(volume, dir, from, FileMode::ReadWrite)?;
    let info: Box<FileInfo> = file
        .get_boxed_info()
        .map_err(firmware("reading the image's directory entry"))?;
    let new_name = cstring(to)?;
    // FileInfo is built in place in an aligned buffer: a byte vector with
    // room to start at the alignment it needs.
    let align = FileInfo::alignment();
    // EFI_FILE_INFO is 80 bytes before its UCS-2 name and terminator.
    let size = 80 + 2 * (to.len() + 1);
    let mut storage = vec![0u8; size + align];
    let offset = storage.as_ptr().align_offset(align);
    let aligned = storage.get_mut(offset..).ok_or(Error::Firmware(
        "aligning the directory entry",
        Status::BUFFER_TOO_SMALL,
    ))?;
    let renamed = FileInfo::new(
        aligned,
        info.file_size(),
        info.physical_size(),
        *info.create_time(),
        *info.last_access_time(),
        *info.modification_time(),
        info.attribute(),
        &new_name,
    )
    .map_err(|_| Error::Firmware("building the new directory entry", Status::BUFFER_TOO_SMALL))?;
    file.set_info(renamed)
        .map_err(firmware("renaming the image"))?;
    file.flush().map_err(firmware("writing the rename"))?;
    Ok(())
}

fn open_file(
    volume: &mut Directory,
    dir: &str,
    name: &str,
    mode: FileMode,
) -> Result<RegularFile, Error> {
    let path = cstring(&format!("{dir}\\{name}"))?;
    volume
        .open(&path, mode, FileAttribute::empty())
        .map_err(firmware("opening the image"))?
        .into_regular_file()
        .ok_or(Error::Firmware(
            "the image is a directory",
            Status::NOT_FOUND,
        ))
}

/// A boot loader interface variable, as systemd-boot sets it: UTF-16 with
/// its terminator, volatile, for this boot only. A variable that cannot be
/// set costs the OS some information, not the boot.
fn set_variable(name: &str, value: &str) {
    let (Ok(name), Ok(value)) = (CString16::try_from(name), CString16::try_from(value)) else {
        return;
    };
    let bytes: Vec<u8> = value
        .to_u16_slice_with_nul()
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let _ = runtime::set_variable(
        &name,
        &LOADER_VENDOR,
        VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS,
        &bytes,
    );
}

/// Whether a key is down as hideBoot starts: one already in the buffer, or
/// one that arrives in the next fifth of a second.
fn key_held() -> bool {
    let pressed = || uefi::system::with_stdin(|input| matches!(input.read_key(), Ok(Some(_))));
    if pressed() {
        return true;
    }
    boot::stall(Duration::from_millis(200));
    pressed()
}

/// The entries, newest first, and a choice: a number, or the arrows and
/// Enter. Returns the index to boot from.
fn menu(entries: &[(&str, Entry)]) -> usize {
    let mut selected = 0usize;
    loop {
        println!();
        println!("hideBoot");
        println!();
        for (i, (dir, entry)) in entries.iter().enumerate() {
            let marker = if i == selected { ">" } else { " " };
            let state = match entry.counter {
                _ if *dir == RECOVERY_DIR => "  (recovery)",
                None => "",
                Some(c) if c.left == 0 => "  (failed to boot)",
                Some(_) => "  (being tried)",
            };
            println!("{marker} {}  {}{state}", i + 1, entry.id);
        }
        println!();
        println!("Up and Down to choose, Enter to boot.");
        let Some(key) = wait_key() else {
            return selected;
        };
        match key {
            Key::Special(ScanCode::UP) => selected = selected.saturating_sub(1),
            Key::Special(ScanCode::DOWN) => {
                selected = (selected + 1).min(entries.len().saturating_sub(1));
            }
            Key::Printable(c) => {
                let c = char::from(c);
                if c == '\r' || c == '\n' {
                    return selected;
                }
                if let Some(n) = c.to_digit(10) {
                    let n = n as usize;
                    if (1..=entries.len()).contains(&n) {
                        return n - 1;
                    }
                }
            }
            _ => {}
        }
    }
}

fn wait_key() -> Option<Key> {
    let event = uefi::system::with_stdin(|input| input.wait_for_key_event().ok())?;
    let events = [event];
    boot::wait_for_event(&events).ok()?;
    uefi::system::with_stdin(|input| input.read_key().ok().flatten())
}

fn cstring(text: &str) -> Result<CString16, Error> {
    CString16::try_from(text)
        .map_err(|_| Error::Firmware("a name UEFI cannot hold", Status::INVALID_PARAMETER))
}
