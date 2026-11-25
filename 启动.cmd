@echo off
setlocal
chcp 65001 >nul
cd /d "%~dp0"
call npm start
set "codex_mobile_exit=%errorlevel%"
if not "%codex_mobile_exit%"=="0" pause
exit /b %codex_mobile_exit%
