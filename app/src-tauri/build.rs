fn main() {
    emit_build_metadata();
    // Phase 65 (build reliability): re-run the build script — and thus
    // re-embed the frontend via generate_context! — whenever the built
    // `dist/` changes. Without this, a pure-frontend change (no .rs edit)
    // could leave the OLD frontend embedded in the binary: the symptom
    // was build #5 shipping stale JS (the new wheel diagnostics never
    // appeared). Belt-and-suspenders over tauri_build's own watching.
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}

// Phase 8.E: emit `YMUX_GIT_HASH` and `YMUX_BUILD_TIME` so the dev
// introspection RPC can show what's running. Falls back to "unknown" if git
// isn't available (e.g. building from a tarball).
fn emit_build_metadata() {
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=YMUX_GIT_HASH={hash}");

    // The COMMIT time, not the wall clock. `SystemTime::now()` here changed
    // the emitted value on every build-script run, which forced a full
    // recompile of the consuming crate each time -- and for the CLI it made
    // the staged binary differ byte-for-byte between two builds of identical
    // source, contradicting build-linux-cli.ps1's own reproducibility claim
    // and dirtying the app crate through include_bytes!. Pairs naturally with
    // YMUX_GIT_HASH: both now answer "what source is this?".
    let build_time = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ct"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    println!("cargo:rustc-env=YMUX_BUILD_TIME={build_time}");

    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
