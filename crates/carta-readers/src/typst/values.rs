//! Owned value sequences that grow the stack for recursive operations.

use std::fmt;
use std::ops::{Deref, DerefMut};

pub(super) struct Values<T>(Vec<T>);

impl<T> From<Vec<T>> for Values<T> {
    fn from(values: Vec<T>) -> Self {
        Self(values)
    }
}

impl<T> Default for Values<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> Deref for Values<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Values<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> IntoIterator for Values<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        std::mem::take(&mut self.0).into_iter()
    }
}

impl<T: Clone> Clone for Values<T> {
    fn clone(&self) -> Self {
        carta_core::with_stack(|| Self(self.0.clone()))
    }
}

impl<T: PartialEq> PartialEq for Values<T> {
    fn eq(&self, other: &Self) -> bool {
        carta_core::with_stack(|| self.0 == other.0)
    }
}

impl<T: fmt::Debug> fmt::Debug for Values<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        carta_core::with_stack(|| self.0.fmt(formatter))
    }
}

impl<T> Drop for Values<T> {
    fn drop(&mut self) {
        carta_core::with_stack(|| drop(std::mem::take(&mut self.0)));
    }
}

impl<T> From<Values<T>> for Vec<T> {
    fn from(mut values: Values<T>) -> Self {
        std::mem::take(&mut values.0)
    }
}

impl<T> FromIterator<T> for Values<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Self(items.into_iter().collect())
    }
}

impl<'a, T> IntoIterator for &'a Values<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
