@echo off
rem Runs the cross-built Windows binary from a LOCAL disk, never from the repo.
rem
rem The repo is mounted over SMB, and a PE image is not loaded up front: it is
rem demand-paged from its file for the whole life of the process. So a cold code
rem page -- a dialog, a teardown path -- is read off the share the first time it
rem runs, and if the share cannot serve it, exception dispatch cannot unwind,
rem faults again, and the process dies of a stack overflow inside ntdll with the
rem real cause nowhere on the stack.
rem
rem Usage: run-local.cmd [release|debug] [args for uniform...]
setlocal EnableExtensions

set "PROFILE=%~1"
if "%PROFILE%"=="" set "PROFILE=release"
shift

set "SRC=%~dp0target\x86_64-pc-windows-msvc\%PROFILE%"
set "DST=%LOCALAPPDATA%\uniform\%PROFILE%"

if not exist "%SRC%\uniform.exe" (
    echo run-local: %SRC%\uniform.exe not found -- build it first ^(cargo xb -r^).
    exit /b 1
)

rem The .pdb comes along or panic backtraces lose their symbols; robocopy
rem skips both when they are already current, so this is only slow after a
rem rebuild. Overwriting a still-running copy fails here rather than corrupting
rem it -- on NTFS the loader holds the image, which is the protection the share
rem does not give us.
robocopy "%SRC%" "%DST%" uniform.exe uniform.pdb /njh /njs /ndl /nc /ns >nul
if errorlevel 8 (
    echo run-local: could not refresh %DST% ^(is a copy of uniform still running?^)
    exit /b 1
)

rem Forward the remaining arguments. `%*` is deliberately not used: it ignores
rem `shift`, so it would hand the profile name to uniform as a font directory.
set "ARGS="
:collect
if "%~1"=="" goto run
set "ARGS=%ARGS% "%~1""
shift
goto collect

:run
rem No `cd`: the working directory stays the repo, so a relative font-directory
rem argument still resolves the way it used to.
"%DST%\uniform.exe"%ARGS%
exit /b %ERRORLEVEL%
