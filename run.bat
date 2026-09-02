@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul

where py >nul 2>&1
if %ERRORLEVEL%==0 (
    py -3.12 run.py %*
    goto :end
)

where python >nul 2>&1
if %ERRORLEVEL%==0 (
    python run.py %*
    goto :end
)

echo No se encontro Python. Instala Python 3.12 desde https://www.python.org/downloads/
echo Marca la opcion "Add python.exe to PATH" e instala el launcher py.
exit /b 1

:end
if errorlevel 1 (
    echo.
    echo El servidor se detuvo con un error. Revisa el mensaje anterior.
    pause
)
