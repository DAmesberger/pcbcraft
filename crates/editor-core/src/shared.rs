//! Copy-on-write sharing of document sections between undo steps.
//!
//! A [`Shared`] section reads like the value it holds and is written in
//! place through `DerefMut`, which copies it first only while another undo
//! step still holds it. Cloning a project therefore copies only handles,
//! and an edit copies only the sections it writes. The document types stay
//! the single model: history entries are whole documents, not diffs.
//! Serialization and schemas are those of the inner value, so files are
//! unchanged.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

/// A document section shared between undo steps until one of them writes it.
#[derive(Default)]
pub struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Whether both handles are the same, unchanged section.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Writing a section first copies it if another step still shares it.
/// Read through `Deref` where a write may not happen.
impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Shared<T> {
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&**self).into_iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut Shared<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&mut **self).into_iter()
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other) || *self.0 == *other.0
    }
}

impl<T: Eq> Eq for Shared<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Serialize> Serialize for Shared<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Shared<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self::new)
    }
}

impl<T: JsonSchema> JsonSchema for Shared<T> {
    fn inline_schema() -> bool {
        T::inline_schema()
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        T::schema_name()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        T::schema_id()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        T::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::Shared;

    #[test]
    fn a_clone_shares_until_written_and_then_copies_once() {
        let a: Shared<Vec<u32>> = vec![1, 2].into();
        let mut b = a.clone();
        assert!(Shared::ptr_eq(&a, &b));
        b.push(3);
        assert!(!Shared::ptr_eq(&a, &b));
        assert_eq!((a.len(), b.len()), (2, 3));
        let before = std::ptr::from_ref::<Vec<u32>>(&b);
        b.push(4);
        assert_eq!(
            std::ptr::from_ref::<Vec<u32>>(&b),
            before,
            "an unshared section is written in place"
        );
    }

    #[test]
    fn serialization_is_that_of_the_inner_value() -> serde_json::Result<()> {
        let a: Shared<Vec<u32>> = vec![1, 2].into();
        assert_eq!(serde_json::to_string(&a)?, "[1,2]");
        let b: Shared<Vec<u32>> = serde_json::from_str("[1,2]")?;
        assert_eq!(a, b);
        Ok(())
    }
}
