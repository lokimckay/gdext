/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use std::env;
use std::process::Command;

fn main() {
    let godot_bin = env::var("GODOT4_BIN").expect("env var 'GODOT4_BIN' not found");

    let output = Command::new(godot_bin.clone())
        .arg("--version")
        .output()
        .unwrap_or_else(|_| panic!("failed to invoke Godot executable '{}'", godot_bin));

    if !output.status.success() {
        panic!("failed to read Godot version from {}", godot_bin);
    }

    let output = String::from_utf8(output.stdout).expect("convert Godot version to UTF-8");
    println!("Godot version: {output}");
}
