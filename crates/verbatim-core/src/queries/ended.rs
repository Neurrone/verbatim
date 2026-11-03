//! The generated test's check that no state names an ended owner: every
//! outpost generation id within a value is found by serialising it through
//! [`Finder`], which sees each [`OutpostId`] by its type's name, whether it
//! stands alone, in a node id, in a tuple or as a map's key, so a field
//! added to the state is checked with no change here.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use verbatim_model::OutpostId;

/// The paths within `value` at which an id of a generation in `ended`
/// stands: fields by name, a list's element as `[]`, a map's entry by its
/// key when the key is text and as `{}` otherwise, an enum's variant by
/// its name, and a map's key itself as `{}` after the map's path.
pub(super) fn naming(value: &impl Serialize, ended: &BTreeSet<OutpostId>) -> BTreeSet<String> {
    let mut finder = Finder {
        ended,
        path: String::new(),
        variants: Vec::new(),
        key: None,
        found: BTreeSet::new(),
    };
    value.serialize(&mut finder).expect("the state serializes");
    finder.found
}

/// The serializer that finds the ids.
struct Finder<'a> {
    ended: &'a BTreeSet<OutpostId>,
    path: String,
    /// Where the path stood before each variant whose fields are being
    /// serialized, innermost last.
    variants: Vec<usize>,
    /// The path segment of the map entry whose value comes next.
    key: Option<String>,
    found: BTreeSet<String>,
}

impl Finder<'_> {
    /// Adds `segment` to the path: an element's or an entry's mark as it
    /// is, a name after a dot.
    fn push(&mut self, segment: &str) {
        if !segment.starts_with(['[', '{']) && !self.path.is_empty() {
            self.path.push('.');
        }
        self.path.push_str(segment);
    }

    /// Serializes `value` at the path with `segment` added.
    fn under<T: Serialize + ?Sized>(&mut self, segment: &str, value: &T) -> Result<(), Never> {
        let at = self.path.len();
        self.push(segment);
        let result = value.serialize(&mut *self);
        self.path.truncate(at);
        result
    }

    /// Enters the fields of `variant`, until [`Finder::leave`].
    fn enter(&mut self, variant: &str) {
        self.variants.push(self.path.len());
        self.push(variant);
    }

    /// Leaves the variant entered last.
    fn leave(&mut self) {
        if let Some(at) = self.variants.pop() {
            self.path.truncate(at);
        }
    }
}

/// The finder's error, which it never raises itself.
#[derive(Debug)]
struct Never(String);

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Never {}

impl ser::Error for Never {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

impl ser::Serializer for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, _: bool) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_f32(self, _: f32) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_f64(self, _: f64) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_char(self, _: char) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_str(self, _: &str) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Never> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<(), Never> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        if name == "OutpostId" {
            let id = serde_json::to_value(value)
                .ok()
                .and_then(|id| id.as_u64())
                .map(OutpostId);
            if id.is_some_and(|id| self.ended.contains(&id)) {
                self.found.insert(self.path.clone());
            }
            return Ok(());
        }
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        self.under(variant, value)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self, Never> {
        self.enter(variant);
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Never> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self, Never> {
        self.enter(variant);
        Ok(self)
    }
}

impl SerializeSeq for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        self.under("[]", value)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

impl SerializeTuple for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        self.under("[]", value)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

impl SerializeTupleStruct for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        self.under("[]", value)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

impl SerializeTupleVariant for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        self.under("[]", value)
    }
    fn end(self) -> Result<(), Never> {
        self.leave();
        Ok(())
    }
}

impl SerializeMap for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Never> {
        self.key = Some(match serde_json::to_value(key) {
            Ok(serde_json::Value::String(name)) => name,
            _ => "{}".to_owned(),
        });
        self.under("{}", key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Never> {
        let key = self.key.take().unwrap_or_else(|| "{}".to_owned());
        self.under(&key, value)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

impl SerializeStruct for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        self.under(name, value)
    }
    fn end(self) -> Result<(), Never> {
        Ok(())
    }
}

impl SerializeStructVariant for &mut Finder<'_> {
    type Ok = ();
    type Error = Never;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        self.under(name, value)
    }
    fn end(self) -> Result<(), Never> {
        self.leave();
        Ok(())
    }
}
