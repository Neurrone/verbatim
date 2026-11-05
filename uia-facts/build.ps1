# Builds the UIA facts workspace (uia-facts/readme.md), locally and in
# .github/workflows/uia-facts.yml, in three steps:
# 1. Derives Cargo.lock from the main root's: copies it and lets
#    `cargo metadata` resolve, which keeps every locked version and drops
#    the entries this workspace does not use. Every later step is --locked,
#    so a re-resolution fails the build instead of adding a second version.
# 2. Builds check-copies alone and runs it: the copied tables, the locks
#    and the workflows' env. It runs before the full build, so a drifted
#    copy fails before a second `windows` is built; it depends on
#    workspace-hack, so its toml is the one the full build reuses.
# 3. Builds the provider program, every fact binary and check-copies.
# Each native command's exit code is checked; Windows PowerShell would
# otherwise stop on the progress cargo writes to standard error.
Push-Location $PSScriptRoot
try {
    Copy-Item -Path ..\Cargo.lock -Destination Cargo.lock -Force -ErrorAction Stop
    cargo metadata --format-version 1 > $null
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }

    cargo build --locked -p check-copies
    if ($LASTEXITCODE -ne 0) { throw "check-copies does not build" }
    & ..\target\debug\check-copies.exe
    if ($LASTEXITCODE -ne 0) { throw "check-copies failed" }

    cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}
