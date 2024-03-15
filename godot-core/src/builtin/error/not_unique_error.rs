/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::obj::{Gd, Inherits};
use std::fmt;

/// Error stemming from the non-uniqueness of a [`Gd`] instance.
///
/// Ensuring uniqueness of a [`Gd`] smart pointer allows to tighten invariants. For example, calling `bind()`/`bind_mut()` on such pointers
/// will always succeed, since there is no other possible reference that could hold a lock.
///
/// Only applicable to [`GodotClass`] objects that inherit from [`RefCounted`](crate::gen::classes::RefCounted). To check the
/// uniqueness, call [`Gd::try_to_unique()`].
///
/// ## Example
///
/// ```no_run
/// use godot::prelude::*;
/// use godot::builtin::error::NotUniqueError;
///
/// let shared = RefCounted::new_gd();
/// let cloned = shared.clone();
/// let result = shared.try_to_unique();
///
/// assert!(result.is_err());
///
/// if let Err(error) = result {
///     assert_eq!(error.get_reference_count(), 2)
/// }
/// ```
#[derive(Debug)]
pub struct NotUniqueError {
    reference_count: i32,
}

impl NotUniqueError {
    // See Gd::try_to_unique().
    pub(crate) fn check<T>(rc: Gd<T>) -> Result<Gd<T>, Self>
    where
        T: Inherits<crate::gen::classes::RefCounted>,
    {
        let rc = rc.upcast::<crate::gen::classes::RefCounted>();
        let reference_count = rc.get_reference_count();

        if reference_count != 1 {
            Err(Self { reference_count })
        } else {
            Ok(rc.cast::<T>())
        }
    }

    /// Get the detected reference count
    pub fn get_reference_count(&self) -> i32 {
        self.reference_count
    }
}

impl std::error::Error for NotUniqueError {}

impl fmt::Display for NotUniqueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pointer is not unique, current reference count: {}",
            self.reference_count
        )
    }
}
