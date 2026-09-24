---
name: recordscreen-control
description: Control the RecordScreen Windows desktop app through its MCP tools or authenticated loopback HTTP API. Use when an agent needs to inspect capture status, list or select a visible app window, take a screenshot, start or stop an MP4 recording, or connect an MCP client to RecordScreen.
---

# RecordScreen Control

Use this skill to operate the RecordScreen Windows app. A standard per-user installation is at `%LOCALAPPDATA%\Programs\RecordScreen\recordscreen.exe`; a source checkout may instead run `target\release\recordscreen.exe`. Prefer its MCP tools when they are available. Otherwise use the authenticated HTTP API on `127.0.0.1:17321`.

## Start-up and safety

1. Make sure the user has started the RecordScreen GUI. The local API and MCP stdio process depend on the GUI process being open.
2. Before taking a screenshot or starting a recording, confirm the user has requested that capture in the current task. Captures may contain private desktop content.
3. If the requested source is unclear, ask whether to capture the whole desktop or a particular app window. Do not guess a window from its title.
4. Check current status before changing a target or stopping a recording. Do not stop a recording unless the user asked to stop it or this agent started it as part of the current task.
5. Use `list_windows` first, then pass an HWND returned by that call. Pass `null` to select the whole desktop. Never invent or reuse a stale HWND.
6. Report the returned output path and whether the API confirmed success. Do not claim a screenshot or recording exists unless the tool/API returned success and the file is present when local file access is available.

## MCP workflow

Use these tools if the user's MCP client has connected to RecordScreen:

| Tool | Purpose |
| --- | --- |
| `get_status` | Read recording state, output folder, FFmpeg availability, and selected source. |
| `list_windows` | List visible, non-minimized app windows with title, PID, and HWND. |
| `select_window` | Lock capture to a listed HWND, or pass `null` for the entire desktop. Cannot change while recording. |
| `take_screenshot` | Save a PNG of the selected source. |
| `start_recording` | Start MP4 capture; optional `fps` must be 10–60 and defaults to 30. |
| `stop_recording` | Stop capture and finalize the MP4. |

Typical requested app-window capture:

1. Call `get_status` and `list_windows`.
2. If needed, ask the user which listed window to use; call `select_window` with its exact integer `hwnd`.
3. Call `take_screenshot`, or call `start_recording` with the requested FPS.
4. For a recording, call `stop_recording` at the user-requested end time and report the returned file path.

The desktop app's own selector can also switch between **Entire desktop** and a listed app window. The same source is used for screenshots and recordings. The selected source is locked while recording.

The GUI stores language, output folder, and frame rate in `%LOCALAPPDATA%\RecordScreen\setting.json`. It creates the file on first launch. After editing these values in the GUI, click **Apply settings** before capture; pending changes are not used for capture. Window HWNDs are session-specific and are not saved to the file.

## HTTP API fallback

Use PowerShell only on the local machine. The bearer token is stored at `%LOCALAPPDATA%\RecordScreen\agent-token.txt`; keep it in memory for the request and never print it, include it in logs, commit it, or send it to a remote service.

```powershell
$token = (Get-Content "$env:LOCALAPPDATA\RecordScreen\agent-token.txt" -Raw).Trim()
$headers = @{ Authorization = "Bearer $token" }
$base = 'http://127.0.0.1:17321'

# Status and visible capture targets
$status = Invoke-RestMethod "$base/api/v1/status" -Headers $headers
$windows = Invoke-RestMethod "$base/api/v1/windows" -Headers $headers
$windows.windows | Select-Object title, process_id, hwnd

# Select one listed window. Replace the title below with the user's chosen title.
$window = $windows.windows | Where-Object title -eq 'Chosen app window' | Select-Object -First 1
if (-not $window) { throw 'The requested window is not in the current window list.' }
$body = @{ hwnd = [long]$window.hwnd } | ConvertTo-Json -Compress
Invoke-RestMethod "$base/api/v1/target" -Method Post -Headers $headers `
  -ContentType 'application/json' -Body $body

# Use this instead to return to the whole desktop:
Invoke-RestMethod "$base/api/v1/target" -Method Post -Headers $headers `
  -ContentType 'application/json' -Body '{"hwnd":null}'

# After explicit user authorization to capture:
Invoke-RestMethod "$base/api/v1/screenshot" -Method Post -Headers $headers
Invoke-RestMethod "$base/api/v1/recording/start" -Method Post -Headers $headers `
  -ContentType 'application/json' -Body '{"fps":30}'
Invoke-RestMethod "$base/api/v1/recording/stop" -Method Post -Headers $headers
```

`GET /health` is unauthenticated and returns `ok`; all `/api/v1/*` endpoints require the bearer token. Only connect to `127.0.0.1`. Common responses include `401` for a missing/invalid token, `404` for a stale or unavailable HWND, and `409` when changing the source during an active recording.

## MCP client setup

Keep the desktop GUI running. Add this server entry to the MCP client's configuration, using the actual path to `recordscreen.exe`. For the standard installation, run `Join-Path $env:LOCALAPPDATA 'Programs\RecordScreen\recordscreen.exe'` in PowerShell to get the path; MCP JSON usually does not expand `%LOCALAPPDATA%`:

```json
{
  "mcpServers": {
    "recordscreen": {
      "command": "C:\\Users\\YOUR_USER\\AppData\\Local\\Programs\\RecordScreen\\recordscreen.exe",
      "args": ["--mcp"]
    }
  }
}
```

Restart or reload the MCP client and verify that `get_status` and `list_windows` are available. The MCP helper reads the local token itself and forwards requests to the GUI's loopback API; do not add the token to MCP configuration.

The installer is per-user and defaults to `%LOCALAPPDATA%\Programs\RecordScreen`; no administrator rights are needed. Setup checks PATH and common WinGet/Scoop/Chocolatey locations. If it cannot find FFmpeg, the wizard checks the Gyan.dev FFmpeg 9.0.2 essentials download by default; the download option remains visible so it can be selected manually if detection disagrees with the app. In unattended setup, pass `/DOWNLOADFFMPEG` to force the download. Setup verifies its pinned SHA-256 and extracts it under the install directory's `_ffmpeg_runtime` folder. Setup does not change PATH; uninstall removes this private runtime folder. If setup skips the download, install an FFmpeg build that supports `gdigrab` and `libx264` on PATH or beside the executable. Replace `YOUR_USER` in the example with the Windows profile folder name.

## Capture limits

- The target app must remain open and not minimized. A protected surface may render black, and app-specific rendering can affect capture results.
- FFmpeg must be available through PATH, WinGet, Scoop, another supported common install location, beside `recordscreen.exe`, or the installer's `_ffmpeg_runtime` folder; it must support `gdigrab` and `libx264`.
- The API and MCP helper do not provide remote/network capture. The API rejects non-loopback requests.
- The app captures video only; it does not record system or microphone audio.

For the complete setup and endpoint reference, see the repository's `README.md`.
