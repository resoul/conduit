use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, Range};
use std::sync::Arc;

/// String contents behind Obj::Str's thin Arc pointer. Shared ranges avoid
/// allocating and copying a character buffer for every line of a text file.
#[derive(Clone)]
pub struct VmString {
    storage: Storage,
}

#[derive(Clone)]
enum Storage {
    Owned(String),
    Shared {
        text: Arc<String>,
        range: Range<usize>,
    },
}

impl VmString {
    pub(crate) fn shared(text: Arc<String>, range: Range<usize>) -> Self {
        assert!(
            text.get(range.clone()).is_some(),
            "invalid UTF-8 string range"
        );
        Self {
            storage: Storage::Shared { text, range },
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        match &self.storage {
            Storage::Owned(text) => text,
            Storage::Shared { text, range } => {
                // SAFETY: shared() checks bounds and UTF-8 boundaries. Storage
                // is private, and the Arc keeps this String immutable and alive.
                // make_owned_mut() replaces a shared view before any mutation.
                unsafe { text.get_unchecked(range.clone()) }
            }
        }
    }

    /// Used by each_line to reuse a uniquely owned string buffer. Shared views
    /// must become owned before mutation, preserving every other string's value.
    pub(crate) fn make_owned_mut(&mut self) -> &mut String {
        if let Storage::Shared { text, range } = &self.storage {
            self.storage = Storage::Owned(text[range.clone()].to_owned());
        }
        let Storage::Owned(text) = &mut self.storage else {
            unreachable!()
        };
        text
    }
}

impl From<String> for VmString {
    fn from(text: String) -> Self {
        Self {
            storage: Storage::Owned(text),
        }
    }
}

impl From<&str> for VmString {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

impl Deref for VmString {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for VmString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for VmString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl fmt::Debug for VmString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

// Compare and hash contents, never the storage variant or backing-file range.
impl PartialEq for VmString {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<str> for VmString {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl Eq for VmString {}

impl PartialOrd for VmString {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VmString {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for VmString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, hash_map::DefaultHasher};

    #[test]
    fn shared_strings_use_content_for_equality_hash_and_order() {
        let text = Arc::new("prefix:Привет:tail".to_owned());
        let shared = VmString::shared(Arc::clone(&text), 7..19);
        let owned = VmString::from("Привет");
        let other = VmString::shared(Arc::new("Привет!".to_owned()), 0..12);
        assert_eq!(shared.as_ptr(), text[7..].as_ptr());
        assert_eq!(shared, owned);
        assert_eq!(shared, other);
        assert_eq!(shared.cmp(&owned), Ordering::Equal);
        for value in [&shared, &owned, &other] {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            let mut reference = DefaultHasher::new();
            "Привет".hash(&mut reference);
            assert_eq!(hasher.finish(), reference.finish());
        }
        let mut map = HashMap::new();
        map.insert(shared, 1);
        map.insert(owned, 2);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&other), Some(&2));
        let mut strings = vec![VmString::from("z"), other, VmString::from("a")];
        strings.sort();
        assert_eq!(
            strings.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["a", "z", "Привет"]
        );
    }

    #[test]
    fn shared_string_survives_its_source_and_materializes_before_mutation() {
        let text = Arc::new("before\nline\nafter".to_owned());
        let mut line = VmString::shared(Arc::clone(&text), 7..11);
        let snapshot = line.clone();
        drop(text);
        line.make_owned_mut().push('!');
        assert_eq!(line.as_str(), "line!");
        assert_eq!(snapshot.as_str(), "line");
        let empty = VmString::shared(Arc::new("x".to_owned()), 1..1);
        assert!(empty.is_empty());
    }

    #[test]
    #[should_panic(expected = "invalid UTF-8 string range")]
    fn shared_string_rejects_non_character_boundary() {
        VmString::shared(Arc::new("é".to_owned()), 1..2);
    }

    #[test]
    #[should_panic(expected = "invalid UTF-8 string range")]
    fn shared_string_rejects_out_of_bounds_range() {
        VmString::shared(Arc::new("text".to_owned()), 0..5);
    }
}
