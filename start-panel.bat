@echo off
cd /d "%~dp0"
title Pumpkin Panel
echo.
echo   Starting Pumpkin Panel...
echo   Open http://localhost:8080 in your browser.
echo   Close this window to stop the panel.
echo.
"%~dp0target\release\pumpkin-panel.exe"
pause
