use std::process::Command;

fn main() {
    let godot_bin = std::env::var("GODOT4_BIN").expect("GODOT4_BIN not set");

    let output = Command::new(godot_bin.clone())
        .arg("--version")
        .output()
        .unwrap_or_else(|_| panic!("failed to invoke Godot executable '{godot_bin}'"));

    if !output.status.success() {
        panic!("failed to read Godot version from {godot_bin}");
    }

    println!("\n======== GODOT VERSION ========");
    print!(
        "{}",
        String::from_utf8(output.stdout).expect("invalid UTF-8")
    );
    println!("===============================");
}
