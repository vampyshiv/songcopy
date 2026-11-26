@echo off
rem ===========================================================================
rem  Song Copy for Windows: find songs on your music server (dahlias-den) and
rem  copy them into a folder on THIS computer (e.g. your Traktor folder).
rem
rem  Double-click to run. It opens in your web browser.
rem  It only ever COPIES from the server. Nothing there is moved or deleted.
rem
rem  Finds the server by itself, on any network: last address that worked ->
rem  dahlias-den.local -> a quick scan of this network -> asks you.
rem ===========================================================================
setlocal EnableExtensions
title Song Copy
cd /d "%USERPROFILE%"

rem ---- settings (optional) -----------------------------------------------------
rem  Folder to fill in automatically, e.g.  set "TRAKTOR=D:\Traktor\Music"
rem  (Leave empty: the page remembers whatever folder you pick the first time.)
set "TRAKTOR="
rem -----------------------------------------------------------------------------

set "MEMO=%LOCALAPPDATA%\songcopy\server.txt"
if not exist "%LOCALAPPDATA%\songcopy" mkdir "%LOCALAPPDATA%\songcopy"

rem ---- 1. Python ---------------------------------------------------------------
:findpython
set "PY="
py -3 -c "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)" >nul 2>nul && set "PY=py -3"
if not defined PY python -c "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)" >nul 2>nul && set "PY=python"
if not defined PY if exist "%LOCALAPPDATA%\Programs\Python\Launcher\py.exe" set PY="%LOCALAPPDATA%\Programs\Python\Launcher\py.exe" -3
if defined PY goto havepython

echo.
echo  Song Copy needs Python (free, from python.org). It isn't installed yet.
echo.
choice /c YN /m " Install it now"
if errorlevel 2 goto nopython
winget install -e --id Python.Python.3.13 --scope user --accept-package-agreements --accept-source-agreements
if errorlevel 1 goto nopython
echo.
echo  Python is installed.
goto findpython

:nopython
echo.
echo  Please install Python from https://www.python.org/downloads/
echo  (tick "Add python.exe to PATH" during setup), then run Song Copy again.
start "" "https://www.python.org/downloads/"
pause
exit /b 1

:havepython

rem ---- 2. Find the server --------------------------------------------------------
:findserver
echo  Looking for your music server ...
powershell -NoProfile -ExecutionPolicy Bypass -Command "$s=Get-Content -Raw -LiteralPath '%~f0'; iex $s.Substring($s.LastIndexOf('#==FIND-SERVER==#'))" > "%TEMP%\songcopy-find.txt"
set "RESULT="
set /p RESULT=<"%TEMP%\songcopy-find.txt"
if not defined RESULT set "RESULT=ASK "
if "%RESULT:~0,6%"=="FOUND " (
  set "SERVER=%RESULT:~6%"
  goto haveserver
)
set "GUESS=%RESULT:~4%"

echo.
echo  Couldn't open the music on the server automatically.
echo  That's normal the first time on a new network: Windows needs you to sign in once.
echo.
echo  If you don't know the server's address: on the server, open a terminal and run
echo      stacks set-lan-ip
echo  It shows the address, and tells you if file sharing needs this network allowed.
echo.
set "ADDR="
if defined GUESS (set /p "ADDR= Server address [press Enter for %GUESS%]: ") else (set /p "ADDR= Server address, e.g. 192.168.1.50: ")
if not defined ADDR set "ADDR=%GUESS%"
if not defined ADDR goto findserver
echo.
echo  Sign in with your SERVER username and password (it will be remembered):
net use "\\%ADDR%\home" /savecred /persistent:yes
if exist "\\%ADDR%\home\Music\" (
  (echo %ADDR%)>"%MEMO%"
  set "SERVER=%ADDR%"
  goto haveserver
)
echo.
echo  Still can't open \\%ADDR%\home\Music.
echo  Check the address, that the server is on, and run  stacks set-lan-ip  on it.
pause
goto findserver

:haveserver
set "SCRIPT=\\%SERVER%\home\songcopy\songcopy.py"
set "MUSIC=\\%SERVER%\home\Music"

rem ---- 3. A shortcut on the desktop (first time only) --------------------------
for /f "usebackq delims=" %%D in (`powershell -NoProfile -Command "[Environment]::GetFolderPath('Desktop')"`) do set "DESKTOP=%%D"
if defined DESKTOP if not exist "%DESKTOP%\Song Copy.bat" (
  copy /y "%~f0" "%DESKTOP%\Song Copy.bat" >nul 2>nul && echo  Added "Song Copy" to your desktop for next time.
)

rem ---- 4. Go --------------------------------------------------------------------
echo.
echo  Starting Song Copy (music from \\%SERVER%\home\Music) ...
echo  Your browser will open. Keep this window open while you use it.
echo.
if defined TRAKTOR (
  %PY% "%SCRIPT%" serve --music "%MUSIC%" --dest "%TRAKTOR%"
) else (
  %PY% "%SCRIPT%" serve --music "%MUSIC%"
)
if errorlevel 1 pause
exit /b

rem ===========================================================================
rem  Everything below is PowerShell, used by step 2 above (cmd never runs it).
rem  Prints "FOUND <server>" or "ASK <best guess>".
rem ===========================================================================
#==FIND-SERVER==#
$ErrorActionPreference = 'SilentlyContinue'
$memo = Join-Path $env:LOCALAPPDATA 'songcopy\server.txt'
function Say($m) { [Console]::Error.WriteLine("  $m") }

# which of these computers answer on the file-sharing port (all checked at once, ~1.5s)
function Open445($hosts) {
  $tries = foreach ($h in $hosts) {
    $c = New-Object Net.Sockets.TcpClient
    try { [pscustomobject]@{ h = $h; c = $c; t = $c.ConnectAsync($h, 445) } } catch { $c.Dispose() }
  }
  $until = (Get-Date).AddMilliseconds(1500)
  while ((Get-Date) -lt $until -and ($tries | Where-Object { -not $_.t.IsCompleted })) { Start-Sleep -Milliseconds 50 }
  $open = @($tries | Where-Object { $_.t.IsCompleted -and -not $_.t.IsFaulted -and $_.c.Connected } | ForEach-Object { $_.h })
  $tries | ForEach-Object { $_.c.Dispose() }
  return $open
}
function Works($h) { Test-Path -LiteralPath "\\$h\home\songcopy\songcopy.py" }
function Found($h) { Set-Content -LiteralPath $memo -Value $h; [Console]::Out.WriteLine("FOUND $h"); exit }
function Ask($h) { [Console]::Out.WriteLine("ASK $h"); exit }

# 1. the address that worked last time, then the server's own name
$names = @()
if (Test-Path $memo) { $names += (Get-Content $memo | Select-Object -First 1).Trim() }
$names += 'dahlias-den.local', 'dahlias-den'
$names = @($names | Where-Object { $_ } | Select-Object -Unique)
$open = Open445 $names
foreach ($h in $names) { if ($open -contains $h -and (Works $h)) { Found $h } }
# reachable by name but not signed in yet: sign in BY NAME, so it keeps working when the IP changes
foreach ($h in 'dahlias-den.local', 'dahlias-den') { if ($open -contains $h) { Ask $h } }

# 2. scan this network (only real networks: the ones with a router)
Say 'Not at the usual address - scanning this network ...'
$mine = @(Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway } | ForEach-Object { $_.IPv4Address.IPAddress })
$targets = foreach ($ip in $mine) { $b = $ip -replace '\.\d+$', ''; 1..254 | ForEach-Object { "$b.$_" } | Where-Object { $_ -ne $ip } }
$open = Open445 $targets
# the server first if the router knows its name
$ranked = $open | Sort-Object { $n = ''; try { $n = [Net.Dns]::GetHostEntry($_).HostName } catch {}; if ($n -like '*dahlias*') { 0 } else { 1 } }
foreach ($h in $ranked) { if (Works $h) { Found $h } }

if ($ranked) {
  Say ('Computers sharing files on this network: ' + ($ranked -join ', '))
  Ask @($ranked)[0]
} else {
  Say 'No computers sharing files were found on this network. Is the server on and connected?'
  Ask ''
}
