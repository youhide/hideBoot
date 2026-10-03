//! hideBoot's decisions, apart from the firmware: which entries there are,
//! in which order they boot, and what an entry's file is called after an
//! attempt. The UEFI application reads the directory, calls these, and
//! renames. Host-tested; `no_std` with `alloc` so that the application can
//! link it.
//!
//! The convention is the Boot Loader Specification's, with Automatic Boot
//! Assessment's counters:
//!
//! ```text
//! \EFI\Linux\<id>[+<left>[-<done>]].efi
//! ```

#![no_std]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

/// One UKI in `\EFI\Linux`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name without the counter or the extension: what the entry is.
    pub id: String,
    pub counter: Option<Counter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub left: u32,
    pub done: u32,
}

impl Entry {
    /// Parses a file name. `None` for anything that is not `*.efi`, and for
    /// a counter that does not parse — such a file is not hideBoot's to
    /// guess about.
    pub fn parse(file_name: &str) -> Option<Entry> {
        let stem = strip_suffix_ignore_case(file_name, ".efi")?;
        if stem.is_empty() {
            return None;
        }
        let (id, counter) = match stem.rsplit_once('+') {
            Some((id, count)) => {
                let (left, done) = match count.split_once('-') {
                    Some((left, done)) => (parse_number(left)?, parse_number(done)?),
                    None => (parse_number(count)?, 0),
                };
                (id, Some(Counter { left, done }))
            }
            None => (stem, None),
        };
        if id.is_empty() {
            return None;
        }
        Some(Entry {
            id: id.to_string(),
            counter,
        })
    }

    pub fn file_name(&self) -> String {
        match self.counter {
            None => format!("{}.efi", self.id),
            Some(Counter { left, done: 0 }) => format!("{}+{left}.efi", self.id),
            Some(Counter { left, done }) => format!("{}+{left}-{done}.efi", self.id),
        }
    }

    /// Out of attempts: boots only when nothing else will.
    pub fn exhausted(&self) -> bool {
        matches!(self.counter, Some(Counter { left: 0, .. }))
    }

    /// The entry after one more attempt, which is what the file is renamed
    /// to before the attempt is made: a boot that never comes back to say
    /// it worked has used the attempt. `None` for an entry without a
    /// counter — one the OS has marked good — and for one that has none
    /// left, which is not counted further.
    pub fn after_attempt(&self) -> Option<Entry> {
        let counter = self.counter?;
        if counter.left == 0 {
            return None;
        }
        Some(Entry {
            id: self.id.clone(),
            counter: Some(Counter {
                left: counter.left - 1,
                done: counter.done.saturating_add(1),
            }),
        })
    }
}

/// The boot order, first first: entries with attempts left, or none
/// counted, before exhausted ones; within each, the highest version first,
/// comparing the ids as versions — digits as numbers — as systemd-boot does.
pub fn order(entries: &mut [Entry]) {
    entries.sort_by(|a, b| match (a.exhausted(), b.exhausted()) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        _ => version_cmp(&b.id, &a.id),
    });
}

/// Compares two strings as versions: runs of digits by their value, the
/// rest byte by byte. `hideos-minimal-10` is newer than `hideos-minimal-9`.
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    let mut a = Runs::new(a);
    let mut b = Runs::new(b);
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (is_digits(x), is_digits(y)) {
                    (true, true) => {
                        let x = x.trim_start_matches('0');
                        let y = y.trim_start_matches('0');
                        x.len().cmp(&y.len()).then_with(|| x.cmp(y))
                    }
                    _ => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Entries from a directory listing, skipping what is not one.
pub fn entries<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<Entry> {
    names.into_iter().filter_map(Entry::parse).collect()
}

/// Splits a string into runs of ASCII digits and runs of everything else.
struct Runs<'a> {
    rest: &'a str,
}

impl<'a> Runs<'a> {
    fn new(text: &'a str) -> Runs<'a> {
        Runs { rest: text }
    }
}

impl<'a> Iterator for Runs<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let first = self.rest.chars().next()?;
        let digits = first.is_ascii_digit();
        let end = self
            .rest
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(self.rest.len());
        let (run, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(run)
    }
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

fn parse_number(text: &str) -> Option<u32> {
    if !is_digits(text) {
        return None;
    }
    text.parse().ok()
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let split = text.len().checked_sub(suffix.len())?;
    if !text.is_char_boundary(split) {
        return None;
    }
    let (stem, tail) = text.split_at(split);
    tail.eq_ignore_ascii_case(suffix).then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(Entry::file_name).collect()
    }

    #[test]
    fn names_round_trip() {
        for name in [
            "hideos-minimal-12-a1769de2c221.efi",
            "hideos-minimal-12-a1769de2c221+3.efi",
            "hideos-minimal-12-a1769de2c221+2-1.efi",
            "hideos-minimal-12-a1769de2c221+0-3.efi",
            "other.efi",
        ] {
            assert_eq!(Entry::parse(name).unwrap().file_name(), name);
        }
        assert_eq!(
            Entry::parse("linux.EFI").unwrap().file_name(),
            "linux.efi",
            "the extension is matched in any case; written in lower case"
        );
    }

    #[test]
    fn what_is_not_an_entry() {
        for name in ["", ".efi", "+3.efi", "x+a.efi", "x+3-b.efi", "x.conf", "x"] {
            assert_eq!(Entry::parse(name), None, "{name}");
        }
    }

    #[test]
    fn an_attempt_moves_one_from_left_to_done() {
        let entry = Entry::parse("a-13+3.efi").unwrap();
        let entry = entry.after_attempt().unwrap();
        assert_eq!(entry.file_name(), "a-13+2-1.efi");
        let entry = entry.after_attempt().unwrap().after_attempt().unwrap();
        assert_eq!(entry.file_name(), "a-13+0-3.efi");
        assert!(entry.exhausted());
        assert_eq!(entry.after_attempt(), None);
        assert_eq!(Entry::parse("a-13.efi").unwrap().after_attempt(), None);
    }

    #[test]
    fn the_newest_usable_boots_first() {
        let mut list = entries(vec![
            "hideos-minimal-9-aaaa.efi",
            "hideos-minimal-11-cccc+0-3.efi",
            "hideos-minimal-10-bbbb+2-1.efi",
            "BOOTX64.EFI.bak",
        ]);
        order(&mut list);
        assert_eq!(
            names(&list),
            [
                "hideos-minimal-10-bbbb+2-1.efi",
                "hideos-minimal-9-aaaa.efi",
                "hideos-minimal-11-cccc+0-3.efi",
            ]
        );
    }

    #[test]
    fn versions_compare_by_value() {
        assert_eq!(version_cmp("a-10", "a-9"), Ordering::Greater);
        assert_eq!(version_cmp("a-009", "a-9"), Ordering::Equal);
        assert_eq!(version_cmp("a-1.2", "a-1.10"), Ordering::Less);
        assert_eq!(version_cmp("b", "a"), Ordering::Greater);
        assert_eq!(version_cmp("a-1", "a-1-x"), Ordering::Less);
    }
}
