@echo off
rem Construit l'appli puis l'installateur (Rust et Inno Setup 6 requis).
cd /d "%~dp0.."
cargo build --release || exit /b 1
set ISCC="%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
%ISCC% installer\guitar-amp.iss || exit /b 1
echo.
echo Installateur : installer\Output\GuitarAmp-Setup.exe
