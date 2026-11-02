@echo off
rem Plyback - double-click to start (Windows). Run setup.ps1 once first.
cd /d "%~dp0"
if not exist ".venv\Scripts\python.exe" (
  echo Running first-time setup...
  powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" || (pause & exit /b 1)
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" %*
