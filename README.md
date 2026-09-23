# RecordScreen

Windows 10+ 桌面錄影與截圖程式，使用 Rust 建置，FFmpeg 負責擷取畫面與輸出 MP4／PNG。桌面 GUI 啟動時會同時提供只監聽本機的 agent API；另提供 MCP stdio 模式，方便 Codex、Claude Desktop 等 MCP client 將錄影／截圖動作註冊為工具。

## 使用需求

- Windows 10 或更新版本。
- FFmpeg 必須在 `PATH` 中，或與 `recordscreen.exe` 放在同一個資料夾。FFmpeg 需包含 Windows `gdigrab` 輸入裝置與 `libx264` 編碼器。
- 第一次啟動後，API 會監聽 `127.0.0.1:17321`；桌面 app 必須保持開啟，agent 才能呼叫。

預設輸出位置：`%USERPROFILE%\Videos\RecordScreen`。可在 GUI 修改輸出資料夾。錄影預設 30 FPS，可在 GUI 調整 10–60 FPS。介面使用 Windows 已安裝的微軟正黑體／新細明體，修正中文缺字問題。

介面右上方可切換 English／繁體中文。首次啟動預設 English，選擇會儲存至 `%LOCALAPPDATA%\RecordScreen\language.txt`，下次啟動沿用。

「錄影來源」可選整個桌面或指定 app 視窗。清單會顯示標題與 PID；同一 app 的不同視窗可分別選取。指定後，截圖及錄影都只擷取該視窗，即使它被其他視窗遮住也不會改錄整個桌面。目標 app 必須保持開啟且不要最小化；錄影期間不能切換來源。

## 建置與啟動

安裝 Rust stable 與 FFmpeg，然後在本資料夾執行：

```powershell
cargo build --release
.
target\release\recordscreen.exe
```

FFmpeg Windows build 可使用 [gyan.dev builds](https://www.gyan.dev/ffmpeg/builds/)。請將 `bin` 資料夾加入使用者 `PATH`，或複製 `ffmpeg.exe` 至 `recordscreen.exe` 同一目錄。

## Agent HTTP API

Agent 只應連線至 `http://127.0.0.1:17321`。GUI 顯示 bearer token；token 也儲存在 `%LOCALAPPDATA%\RecordScreen\agent-token.txt`。請勿把 token 貼到遠端服務、提交到 Git，或提供給不受信任的 agent。每個控制端點都需要 `Authorization: Bearer <token>`。API 會拒絕非 loopback Host／Origin。

| Method | Path | 功能 |
| --- | --- | --- |
| GET | `/health` | 無認證健康狀態 |
| GET | `/api/v1/status` | 讀取錄影狀態與輸出位置 |
| GET | `/api/v1/windows` | 列出目前可選取的可見 app 視窗與 HWND |
| POST | `/api/v1/target` | 指定視窗 `{ "hwnd": 1234 }`；傳 `{ "hwnd": null }` 切回整個桌面 |
| POST | `/api/v1/screenshot` | 擷取一張 PNG，回傳檔案路徑 |
| POST | `/api/v1/recording/start` | 開始錄影，JSON body 可帶 `{"fps":30}`（10–60） |
| POST | `/api/v1/recording/stop` | 停止錄影並完成 MP4 封裝 |

PowerShell 範例（從 GUI 複製 token 後執行）：

```powershell
$token = Get-Content "$env:LOCALAPPDATA\RecordScreen\agent-token.txt" -Raw
$headers = @{ Authorization = "Bearer $($token.Trim())" }
Invoke-RestMethod http://127.0.0.1:17321/api/v1/status -Headers $headers
Invoke-RestMethod http://127.0.0.1:17321/api/v1/windows -Headers $headers
# 將下列 HWND 換成 /windows 回傳的值；傳 null 可恢復整個桌面
Invoke-RestMethod http://127.0.0.1:17321/api/v1/target -Method Post -Headers $headers -ContentType 'application/json' -Body '{"hwnd":1234}'
Invoke-RestMethod http://127.0.0.1:17321/api/v1/screenshot -Method Post -Headers $headers
Invoke-RestMethod http://127.0.0.1:17321/api/v1/recording/start -Method Post -Headers $headers -ContentType 'application/json' -Body '{"fps":30}'
Invoke-RestMethod http://127.0.0.1:17321/api/v1/recording/stop -Method Post -Headers $headers
```

## MCP agent 接入

先開啟 RecordScreen GUI，再在 MCP client 設定中加入本機 stdio server。以 Codex CLI／桌面可讀取的 MCP 設定格式為例，將路徑換成實際的 release exe：

```json
{
  "mcpServers": {
    "recordscreen": {
      "command": "D:\\recordscreen\\target\\release\\recordscreen.exe",
      "args": ["--mcp"]
    }
  }
}
```

MCP tools：`get_status`、`list_windows`、`select_window`、`take_screenshot`、`start_recording`（`fps` 可選）與 `stop_recording`。`select_window` 使用 `list_windows` 回傳的 HWND；傳入 `null` 可切回整個桌面。stdio 子程序會從同一個使用者設定檔讀取 token，再經過本機 API 呼叫 GUI app。關閉 GUI 後，MCP 工具會回報 API 無法連線。

Release 使用 size optimization、LTO、單一 codegen unit、移除符號與 abort panic；這些設定會讓 release 編譯較久，但不影響 debug build。

## 螢幕與隱私

截圖和錄影會包含目前桌面上可見的內容；開始錄影前請確認畫面適合保存。檔案只寫入設定的本機輸出資料夾。此版本不錄製系統／麥克風音訊，也不提供遠端控制或網路監聽。
