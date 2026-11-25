# Incentium Node for Windows: one command in PowerShell, no administrator rights.
#
#   irm https://git.incentium.org/incentium/incentium-chain/raw/branch/main/scripts/install-node.ps1 | iex
#
# Everything goes into %USERPROFILE%\.incentium-node:
#   * Python (through uv) and llama.cpp's llama-server (CUDA on NVIDIA graphics cards, otherwise CPU),
#   * the Incentium node, which picks the Qwen3.5 model this PC can run, downloads it,
#     asks to join the network and mines; it starts by itself when you log in,
#   * a dashboard at http://127.0.0.1:7777, opened now and from the "Incentium" icon on the desktop.
# Run the same command again to update; your key, model and settings are kept.
# Remove:  $env:INCENTIUM_UNINSTALL=1; irm https://git.incentium.org/incentium/incentium-chain/raw/branch/main/scripts/install-node.ps1 | iex
#
# Works in Windows PowerShell 5.1 (built into Windows 10 and 11) and PowerShell 7.
# Everything runs inside a function and errors are thrown, never "exit", so the window stays open.

function Install-IncentiumNode {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'      # the progress bar makes downloads very slow in 5.1
  try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch {}

  $Repo    = if ($env:INCENTIUM_REPO)    { $env:INCENTIUM_REPO }    else { 'incentium/incentium-chain' }
  $Branch  = if ($env:INCENTIUM_BRANCH)  { $env:INCENTIUM_BRANCH }  else { 'main' }
  $Network = if ($env:INCENTIUM_NETWORK) { $env:INCENTIUM_NETWORK } else { 'testnet' }
  $D       = Join-Path $env:USERPROFILE '.incentium-node'
  $Port    = 7777
  $Startup = Join-Path ([Environment]::GetFolderPath('Startup')) 'Incentium Node.lnk'
  $Icon    = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Incentium.url'

  function Say($msg) { Write-Host ''; Write-Host "==> $msg" -ForegroundColor White }
  function Fail($msg) { throw $msg }
  function Get-File($url, $out) { Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $out }
  function Test-Url($url, $method = 'Head') {
    try { $r = Invoke-WebRequest -UseBasicParsing -Method $method -Uri $url; return ($r.StatusCode -eq 200) } catch { return $false }
  }
  function Stop-IncentiumNode {
    # the launcher, the node and llama-server all run from inside $D
    $mine = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
      ($_.CommandLine -and $_.CommandLine -like "*$D*") -or ($_.ExecutablePath -and $_.ExecutablePath -like "$D*") })
    foreach ($p in $mine) {
      if ($p.ProcessId -ne $PID) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
    }
    if ($mine.Count) { Start-Sleep -Seconds 2 }
  }

  # -------------------------------------------------------------- uninstall
  if ($env:INCENTIUM_UNINSTALL -eq '1') {
    Remove-Item Env:\INCENTIUM_UNINSTALL -ErrorAction SilentlyContinue
    Stop-IncentiumNode
    Remove-Item -Force -ErrorAction SilentlyContinue $Startup, $Icon
    $key = Join-Path $D 'miner-key.json'
    if (Test-Path $key) {
      $backup = Join-Path $env:USERPROFILE 'incentium-miner-key-backup.json'
      Copy-Item $key $backup -Force
      Write-Host "Your miner key was copied to $backup. Keep it if you want your rewards."
    }
    if (Test-Path $D) { Remove-Item -Recurse -Force $D }
    Write-Host 'Incentium Node is removed.'
    return
  }

  # -------------------------------------------------------------- this PC
  $arch = $env:PROCESSOR_ARCHITECTURE
  if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }   # 32-bit PowerShell on 64-bit Windows
  switch ($arch) {
    'AMD64' { $UvAsset = 'uv-x86_64-pc-windows-msvc.zip';  $CpuPlat = 'win-cpu-x64' }
    'ARM64' { $UvAsset = 'uv-aarch64-pc-windows-msvc.zip'; $CpuPlat = 'win-cpu-arm64' }
    default { Fail "unsupported processor: $arch (a 64-bit PC is needed)" }
  }
  $Nvidia = ($arch -eq 'AMD64') -and [bool](Get-Command nvidia-smi -ErrorAction SilentlyContinue)
  $MemGB = [math]::Floor((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
  if ($MemGB -lt 7) { Fail "at least 8 GB of memory is needed, so that mining never takes more than a quarter of it (this PC has $MemGB GB)." }
  New-Item -ItemType Directory -Force -Path $D, "$D\bin", "$D\models", "$D\llama", "$D\src" | Out-Null
  $FreeGB = [math]::Floor((Get-PSDrive ($D.Substring(0, 1))).Free / 1GB)
  if ($FreeGB -lt 3) { Fail "at least 3 GB of free disk is needed (found $FreeGB GB)." }

  Write-Host "Installing Incentium Node in $D. This takes a few minutes."
  Stop-IncentiumNode                                   # an update replaces files the running node uses

  # -------------------------------------------------------------- Python (uv, no admin)
  Say 'Preparing Python'
  $env:UV_PYTHON_INSTALL_DIR = "$D\python"
  $env:UV_CACHE_DIR = "$D\cache"
  $Uv = "$D\bin\uv.exe"
  if (-not (Test-Path $Uv)) {
    $zip = "$D\bin\uv.zip"
    Get-File "https://github.com/astral-sh/uv/releases/latest/download/$UvAsset" $zip
    Expand-Archive -Force $zip "$D\bin"
    Remove-Item $zip
    if (-not (Test-Path $Uv)) { Fail 'could not install uv' }
  }
  $Py  = "$D\venv\Scripts\python.exe"
  $PyW = "$D\venv\Scripts\pythonw.exe"
  if (-not (Test-Path $Py)) {
    & $Uv venv -q --python 3.12 "$D\venv"
    if ($LASTEXITCODE -ne 0) { Fail 'could not prepare Python' }
  }
  & $Uv pip install -q --python $Py substrate-interface
  if ($LASTEXITCODE -ne 0) { Fail 'could not install Python packages' }

  # -------------------------------------------------------------- node code
  Say 'Downloading Incentium Node'
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("incentium-" + [guid]::NewGuid())
  New-Item -ItemType Directory -Path $tmp | Out-Null
  try {
    Get-File "https://git.incentium.org/$Repo/archive/$Branch.zip" "$tmp\src.zip"
    Expand-Archive -Force "$tmp\src.zip" $tmp
    $top = Get-ChildItem -Directory $tmp | Select-Object -First 1
    if (Test-Path "$D\src\neuron") { Remove-Item -Recurse -Force "$D\src\neuron" }
    Move-Item (Join-Path $top.FullName 'neuron') "$D\src\neuron"
  } catch {
    Fail "could not download the code ($($_.Exception.Message))"
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
  if (-not (Test-Path "$D\src\neuron\incentium_neuron\node.py")) { Fail 'the downloaded code has no node' }

  # -------------------------------------------------------------- llama.cpp
  # llama.cpp publishes several releases a day and the Windows files are often uploaded after
  # the release itself, so take the newest release that has the file this PC needs.
  $LlamaDl = 'https://github.com/ggml-org/llama.cpp/releases/download'
  function Find-LlamaRelease($plat) {          # returns a release tag such as b12606, or $null
    $want = "-bin-$plat.zip"
    if ($env:INCENTIUM_LLAMA_TAG -and (Test-Url "$LlamaDl/$($env:INCENTIUM_LLAMA_TAG)/llama-$($env:INCENTIUM_LLAMA_TAG)$want")) {
      return $env:INCENTIUM_LLAMA_TAG
    }
    try {                                      # the GitHub API, newest releases first (can be rate limited)
      $rels = Invoke-RestMethod -UseBasicParsing -Uri 'https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20'
      foreach ($r in $rels) {
        if ($r.assets | Where-Object { $_.name -like "*$want" }) { return $r.tag_name }
      }
    } catch {}
    try {                                      # without the API: step back from the latest release tag
      $req = [Net.HttpWebRequest]::Create('https://github.com/ggml-org/llama.cpp/releases/latest')
      $req.AllowAutoRedirect = $false
      $req.Method = 'HEAD'
      $res = $req.GetResponse()
      $loc = $res.Headers['Location']
      $res.Close()
      if ($loc -match '/b(\d+)$') {
        $n = [int]$Matches[1]
        for ($i = 0; $i -le 40; $i++) {
          $t = "b$($n - $i)"
          if (Test-Url "$LlamaDl/$t/llama-$t$want") { return $t }
        }
      }
    } catch {}
    if (Test-Url "$LlamaDl/b12606/llama-b12606$want") { return 'b12606' }   # a release known to work
    return $null
  }
  function Install-Llama($plat) {              # returns the path to llama-server.exe, or $null
    $tag = Find-LlamaRelease $plat
    if (-not $tag) { return $null }
    if (Test-Path "$D\llama") { Remove-Item -Recurse -Force "$D\llama" }
    New-Item -ItemType Directory -Path "$D\llama" | Out-Null
    Get-File "$LlamaDl/$tag/llama-$tag-bin-$plat.zip" "$D\llama\l.zip"
    Expand-Archive -Force "$D\llama\l.zip" "$D\llama"
    Remove-Item "$D\llama\l.zip"
    $exe = Get-ChildItem -Recurse -File -Filter 'llama-server.exe' "$D\llama" | Select-Object -First 1
    if (-not $exe) { return $null }
    if ($plat -like 'win-cuda-*') {            # the CUDA runtime comes as a separate file, next to llama-server
      $cuda = $plat -replace '^win-cuda-(.+)-x64$', '$1'
      try {
        Get-File "$LlamaDl/$tag/cudart-llama-bin-win-cuda-$cuda-x64.zip" "$D\llama\c.zip"
        Expand-Archive -Force "$D\llama\c.zip" $exe.DirectoryName
        Remove-Item "$D\llama\c.zip"
      } catch { return $null }
    }
    Get-ChildItem -Recurse -File "$D\llama" | Unblock-File -ErrorAction SilentlyContinue
    return $exe.FullName
  }
  function Test-Llama($exe) {
    if (-not $exe -or -not (Test-Path $exe)) { return $false }
    $ErrorActionPreference = 'Continue'        # llama-server prints to stderr; that is not an error here
    try { & $exe --version *> $null; return ($LASTEXITCODE -eq 0) } catch { return $false }
  }

  $Llama = Get-ChildItem -Recurse -File -Filter 'llama-server.exe' "$D\llama" -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
  if (-not (Test-Llama $Llama) -or $env:INCENTIUM_UPDATE_LLAMA -eq '1') {
    Say 'Downloading llama.cpp'
    $Llama = $null
    if ($Nvidia) {                             # NVIDIA card: the CUDA build, if this driver can run it
      $Llama = Install-Llama 'win-cuda-12.4-x64'
      if (-not (Test-Llama $Llama)) {
        Write-Host 'The NVIDIA (CUDA) version did not start here, so the node uses the processor instead.'
        $Llama = $null
      }
    }
    if (-not $Llama) { $Llama = Install-Llama $CpuPlat }
    if (-not $Llama) { Fail "could not find llama.cpp for $CpuPlat (check the internet connection, or set INCENTIUM_LLAMA_TAG)" }
    if (-not (Test-Llama $Llama)) { Fail 'llama-server does not start on this PC' }
  }

  # -------------------------------------------------------------- settings (kept on update)
  Say 'Saving settings'
  $cfgPy = @'
import json, os, secrets, sys
home, network, llama, port = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
p = os.path.join(home, "config.json")
c = json.load(open(p)) if os.path.exists(p) else {}
c.setdefault("token", secrets.token_urlsafe(24))
c.setdefault("network", network)
c["llama_server"], c["port"] = llama, port
with open(p + ".tmp", "w") as f:
    json.dump(c, f, indent=2)
os.replace(p + ".tmp", p)
print(c["token"])
'@
  [IO.File]::WriteAllText("$D\save-config.py", $cfgPy)
  $Token = (& $Py "$D\save-config.py" $D $Network $Llama $Port | Select-Object -Last 1)
  if ($LASTEXITCODE -ne 0 -or -not $Token) { Fail 'could not save the settings' }
  $Token = $Token.Trim()
  $Url = "http://127.0.0.1:$Port/?t=$Token"

  # -------------------------------------------------------------- start at login
  Say 'Starting Incentium Node'
  # A small launcher keeps the node running without a window: it restarts the node 30 seconds
  # after it stops, writes node.log and runs at below-normal priority so the PC stays responsive.
  $launcher = @'
import os, subprocess, sys, time
home = os.path.dirname(os.path.abspath(__file__))
py = os.path.join(home, "venv", "Scripts", "python.exe")
env = dict(os.environ, PYTHONUNBUFFERED="1")
CREATE_NO_WINDOW, BELOW_NORMAL = 0x08000000, 0x00004000
while True:
    with open(os.path.join(home, "node.log"), "ab") as log:
        subprocess.call([py, "-m", "incentium_neuron.node", "--home", home],
                        cwd=os.path.join(home, "src", "neuron"), env=env,
                        stdout=log, stderr=subprocess.STDOUT, creationflags=CREATE_NO_WINDOW | BELOW_NORMAL)
    time.sleep(30)
'@
  [IO.File]::WriteAllText("$D\run-node.pyw", $launcher)
  $sh = New-Object -ComObject WScript.Shell
  $lnk = $sh.CreateShortcut($Startup)
  $lnk.TargetPath = $PyW
  $lnk.Arguments = "`"$D\run-node.pyw`""
  $lnk.WorkingDirectory = $D
  $lnk.Description = 'Incentium Node'
  $lnk.Save()
  Start-Process -FilePath $PyW -ArgumentList "`"$D\run-node.pyw`"" -WorkingDirectory $D

  # -------------------------------------------------------------- desktop icon
  [IO.File]::WriteAllText($Icon, "[InternetShortcut]`r`nURL=$Url`r`n")

  $up = $false
  for ($i = 0; $i -lt 30 -and -not $up; $i++) {
    if (Test-Url $Url 'Get') { $up = $true } else { Start-Sleep -Seconds 1 }
  }
  if (-not $up) {
    $log = "$D\node.log"
    if (Test-Path $log) { Get-Content $log -Tail 20 | Write-Host }
    Fail "the dashboard did not start; see $log"
  }
  if ($env:INCENTIUM_NO_BROWSER -ne '1') { Start-Process $Url }

  Write-Host ''
  Write-Host 'Incentium Node is running.' -ForegroundColor Green
  Write-Host '  Your dashboard has opened in the browser. Later, open it with the "Incentium" icon on your desktop.'
  Write-Host '  It downloads the right model, then asks you to send a short request to join.'
  Write-Host '  You can close this window.'
}

try {
  Install-IncentiumNode
} catch {
  Write-Host ''
  Write-Host "Something went wrong: $($_.Exception.Message)" -ForegroundColor Red
  if ($env:INCENTIUM_CI -eq '1') { throw }
}
