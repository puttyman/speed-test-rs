# Running speed-test-rs on Windows

Download `speed-test-rs-<version>-x86_64-pc-windows-msvc.zip` from the GitHub Release, check
it against `SHA256SUMS`, and unpack `speed-test-rs.exe` to a permanent folder, for example
`C:\Program Files\speed-test-rs\`.

Run it once by hand (`.\speed-test-rs.exe`) and open the printed URL. With no options it
serves on `0.0.0.0:8080` and keeps history in `%APPDATA%\speed-test-rs`.

## Firewall

The first run shows a Windows Defender Firewall prompt. Allow it on **Private** networks only.
To add the rules yourself (elevated PowerShell), open TCP 8080 for the UI, API and WebSocket
tests and UDP 3478 for the WebRTC packet-loss test:

```powershell
New-NetFirewallRule -DisplayName "speed-test-rs (TCP 8080)" -Direction Inbound `
  -Protocol TCP -LocalPort 8080 -Action Allow -Profile Private
New-NetFirewallRule -DisplayName "speed-test-rs (UDP 3478)" -Direction Inbound `
  -Protocol UDP -LocalPort 3478 -Action Allow -Profile Private
```

The same rules with `netsh` (elevated Command Prompt):

```bat
netsh advfirewall firewall add rule name="speed-test-rs (TCP 8080)" dir=in action=allow protocol=TCP localport=8080 profile=private
netsh advfirewall firewall add rule name="speed-test-rs (UDP 3478)" dir=in action=allow protocol=UDP localport=3478 profile=private
```

Check that your network is set to *Private* (Settings → Network & internet → Properties).
On a *Public* network these rules don't apply.

## Running as a service

### NSSM (recommended)

`speed-test-rs.exe` is a normal console program. It does not implement the Windows service
control API, so it needs a service wrapper such as [NSSM](https://nssm.cc/). Run these in an
elevated prompt:

```bat
nssm install speed-test-rs "C:\Program Files\speed-test-rs\speed-test-rs.exe"
nssm set speed-test-rs AppDirectory "C:\Program Files\speed-test-rs"
nssm set speed-test-rs AppEnvironmentExtra SPEEDTEST_DATA_DIR=C:\ProgramData\speed-test-rs SPEEDTEST_LISTEN=0.0.0.0:8080
nssm set speed-test-rs AppStdout C:\ProgramData\speed-test-rs\speed-test-rs.log
nssm set speed-test-rs AppStderr C:\ProgramData\speed-test-rs\speed-test-rs.log
nssm set speed-test-rs Start SERVICE_AUTO_START
nssm start speed-test-rs
```

Set `SPEEDTEST_DATA_DIR` explicitly. Services run as `LocalSystem`, whose `%APPDATA%` is under
`C:\Windows\System32\config\systemprofile`, which is easy to lose track of.

To remove it: `nssm stop speed-test-rs` then `nssm remove speed-test-rs confirm`.

### sc.exe

`sc.exe` can only start programs that implement the service API. Pointed straight at
`speed-test-rs.exe`, the service fails with error 1053 ("did not respond to the start or
control request in a timely fashion"). Use `sc.exe` to manage the NSSM-wrapped service:

```bat
sc.exe query speed-test-rs
sc.exe stop speed-test-rs
sc.exe start speed-test-rs
sc.exe config speed-test-rs start= delayed-auto
```

or wrap the binary with another service host such as WinSW and register that with
`sc.exe create speed-test-rs binPath= "C:\path\to\winsw.exe" start= auto`.

### Task Scheduler (no extra tools)

To run it at startup without a service wrapper (elevated PowerShell):

```powershell
$action  = New-ScheduledTaskAction -Execute "C:\Program Files\speed-test-rs\speed-test-rs.exe"
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -RestartCount 3 `
  -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "speed-test-rs" -Action $action -Trigger $trigger `
  -Settings $settings -User "NT AUTHORITY\NETWORK SERVICE"
```

Run as `NETWORK SERVICE` like this, history goes to that account's profile. Set
`SPEEDTEST_DATA_DIR` as a machine environment variable to choose another folder.
