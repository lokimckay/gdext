/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

// TODO move ConvertError here, once open PRs are resolved.

mod call_error;
mod not_unique_error;

pub use call_error::CallError;
pub use not_unique_error::NotUniqueError;
