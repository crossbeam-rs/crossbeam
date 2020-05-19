set RUSTFLAGS= -C target-feature=+crt-static  -C link-arg=/SUBSYSTEM:CONSOLE,5.01
cargo test   --no-run

if %errorlevel% neq 0 exit /b %errorlevel%
