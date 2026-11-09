@echo off
rem Plyback - double-click to start (Windows). Run setup.ps1 once first.
cd /d "%~dp0"
set "PLYBACK_NEEDS_SETUP="
if not exist "rust\target\release\plyback.exe" set "PLYBACK_NEEDS_SETUP=1"
rem A setup that stopped after the server build leaves no web interface.
if not defined PLYBACK_FRONTEND if not exist "frontend\dist\index.html" set "PLYBACK_NEEDS_SETUP=1"
if defined PLYBACK_NEEDS_SETUP (
  set "PLYBACK_NEEDS_SETUP="
  echo Running setup: the server or the web interface is not built yet...
  powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" || (pause & exit /b 1)
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" %*
