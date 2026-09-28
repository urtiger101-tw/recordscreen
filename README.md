# RecordScreen

Windows 10+ 桌面錄影與截圖程式，使用 Rust 建置，FFmpeg 負責擷取畫面與輸出 MP4／PNG。桌面 GUI 啟動時會同時提供只監聽本機的 agent API；另提供 MCP stdio 模式，方便 Codex、Claude Desktop 等 MCP client 將錄影／截圖動作註冊為工具。

## 使用需求

- Windows 10 或更新版本。
- v0.1.7 起的 x64 安裝包將 Visual C++ 執行階段靜態連結至 RecordScreen，不需另外安裝 VC++ Redistributable。舊版若顯示找不到 `VCRUNTIME140.dll`，請使用 v0.1.7 或更新的安裝包升級。
- FFmpeg 需包含 Windows `gdigrab` 輸入裝置與 `libx264` 編碼器。Windows 安裝程式會先檢查 PATH 與常見安裝位置；找不到時預設提供下載並安裝 FFmpeg。
- 第一次啟動後，API 會監聽 `127.0.0.1:17321`；桌面 app 必須保持開啟，agent 才能呼叫。

預設輸出位置：`%USERPROFILE%\Videos\RecordScreen`。可在 GUI 修改輸出資料夾。錄影預設 30 FPS，可在 GUI 調整 10–60 FPS。修改擷取設定後按「Apply settings／套用設定」才會儲存並套用；介面語系可先預覽，不影響錄影按鈕。介面使用 Windows 已安裝的微軟正黑體／新細明體，修正中文缺字問題。

在「Settings／設定」頁可切換 English／繁體中文。首次啟動預設 English。語系、輸出資料夾、FPS 和錄影倒數秒數儲存於 `%LOCALAPPDATA%\RecordScreen\setting.json`；若檔案不存在，啟動時自動建立。舊版設定檔未含倒數欄位時維持立即開始錄影；從更早版本升級且尚無 JSON 時，會從 `language.txt` 匯入語系。按「Apply settings／套用設定」後，下次啟動沿用。設定檔無法讀取時會顯示錯誤，保留原檔供修復。

主介面分為擷取、檔案庫、設定與 Agent API 四頁。擷取頁提供 0／3／5／10 秒倒數、錄影計時、截圖與輸出資料夾捷徑；`Ctrl+R` 可在 RecordScreen 視窗有焦點時開始／停止錄影，`Ctrl+Shift+S` 可截圖。設定頁可用 Windows 原生資料夾選擇器選取輸出位置。檔案庫列出輸出資料夾中最近 30 個 RecordScreen 產出檔，可在檔案總管定位。Agent API 的 Token 預設隱藏，只有按「Show／顯示」才會出現在畫面上，也可按「Copy token／複製 Token」。

主介面標題會顯示目前 app 版本，方便確認安裝版本與回報問題。程式視窗、執行檔與安裝捷徑使用相同的 RecordScreen 圖示；圖示原始檔位於 `assets/recordscreen.ico`。

v0.1.6 改善設定儲存與 Agent 連線診斷：設定會先寫入同目錄暫存檔並同步，再替換 `setting.json`；寫入或替換失敗時保留原設定。「Discard changes／捨棄變更」可恢復目前已套用的設定。Agent API 頁會顯示啟動、監聽或失敗狀態；連接埠占用或 Token 無法讀寫時會顯示原因。指定視窗以 HWND 與 PID 判斷，切換文件或瀏覽器分頁造成標題改變時仍可擷取；視窗關閉、最小化或已屬於其他程序時須重新選取。

「錄影來源」可選整個桌面或指定 app 視窗。清單會顯示標題與 PID；同一 app 的不同視窗可分別選取。指定後，截圖及錄影都只擷取該視窗，即使它被其他視窗遮住也不會改錄整個桌面。目標 app 必須保持開啟且不要最小化；錄影期間不能切換來源。

## 建置與啟動

安裝 Rust stable 與 FFmpeg，然後在本資料夾執行：

```powershell
cargo build --release
.\scripts\check-runtime-dependencies.ps1
.\target\release\recordscreen.exe
```

`.cargo/config.toml` 為 `x86_64-pc-windows-msvc` 啟用 `+crt-static`，請從專案根目錄建置，避免以環境變數 `RUSTFLAGS` 覆蓋此設定。`build.rs` 會拒絕缺少靜態 CRT 的 MSVC 建置；DLL 檢查腳本會透過 Visual Studio 的 `dumpbin.exe` 檢查產物，發現 VC++ 執行階段 DLL 依賴即停止。CI 在打包前執行同一項檢查。設定依據見 [Rust CRT 靜態連結文件](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes)。

FFmpeg Windows build 可使用 [gyan.dev builds](https://www.gyan.dev/ffmpeg/builds/)。請將 `bin` 資料夾加入使用者 `PATH`，或複製 `ffmpeg.exe` 至 `recordscreen.exe` 同一目錄。

## Windows 安裝程式

從 GitHub Releases 下載 `RecordScreen-Setup-<版本>-x64.exe`，執行後預設安裝至目前使用者的 `%LOCALAPPDATA%\Programs\RecordScreen`，不需要系統管理員權限。開始功能表會建立捷徑；安裝精靈也可選擇建立桌面捷徑。解除安裝可從 Windows「已安裝的應用程式」或開始功能表執行。

安裝程式會先尋找 PATH、WinGet、Scoop、Chocolatey 與常見 FFmpeg 目錄。若找不到，安裝精靈會預設勾選下載 FFmpeg 9.0.2 essentials（約 35 MB）；即使偵測到其他 FFmpeg，仍可手動勾選此項以修復。檔案會從 [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) 以 HTTPS 下載，並在解壓前驗證固定的 SHA-256。FFmpeg 會放在 RecordScreen 安裝目錄的 `_ffmpeg_runtime`，不會修改系統 PATH；解除安裝 RecordScreen 時會一併移除這個目錄。若電腦目前離線，可取消勾選並稍後再安裝 FFmpeg。此下載由 Gyan.dev 提供，該版本依 GPLv3 發佈；本安裝程式不重新散佈 FFmpeg 二進位檔。

無人值守安裝若需強制下載 FFmpeg，可加上 `/DOWNLOADFFMPEG`，例如 `RecordScreen-Setup-0.1.7-x64.exe /VERYSILENT /DOWNLOADFFMPEG`。

自行從原始碼啟動時，仍須先安裝支援 `gdigrab` 與 `libx264` 的 FFmpeg，並放入 PATH 或程式同目錄。設定 MCP 時，server command 預設使用 `%LOCALAPPDATA%\Programs\RecordScreen\recordscreen.exe`。JSON 設定通常不會展開環境變數，請填入實際完整路徑。可用 PowerShell 取得：

```powershell
Join-Path $env:LOCALAPPDATA 'Programs\RecordScreen\recordscreen.exe'
```

此安裝程式目前支援 Windows x64。從原始碼建置安裝包前，需先安裝 Inno Setup 7 並執行 `cargo build --release`，再以 Inno Setup Compiler 編譯 `installer\RecordScreen.iss`。

## Agent HTTP API

Agent 只應連線至 `http://127.0.0.1:17321`。GUI 的 Agent API 頁可按「Show」查看 bearer token；token 也儲存在 `%LOCALAPPDATA%\RecordScreen\agent-token.txt`。請勿把 token 貼到遠端服務、提交到 Git，或提供給不受信任的 agent。每個控制端點都需要 `Authorization: Bearer <token>`。API 會拒絕非 loopback Host／Origin。

| Method | Path | 功能 |
| --- | --- | --- |
| GET | `/health` | 無認證健康狀態 |
| GET | `/api/v1/status` | 讀取錄影狀態、輸出位置、app `version` 與 `process_id` |
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
      "command": "C:\\Users\\YOUR_USER\\AppData\\Local\\Programs\\RecordScreen\\recordscreen.exe",
      "args": ["--mcp"]
    }
  }
}
```

MCP tools：`get_status`、`list_windows`、`select_window`、`take_screenshot`、`start_recording`（`fps` 可選）與 `stop_recording`。`select_window` 使用 `list_windows` 回傳的 HWND；傳入 `null` 可切回整個桌面。stdio 子程序會從同一個使用者設定檔讀取 token，再經過本機 API 呼叫 GUI app。關閉 GUI 後，MCP 工具會回報 API 無法連線。

## Codex Agent skill

專案包含 `.agents/skills/recordscreen-control/SKILL.md`，說明如何透過 MCP 或本機 HTTP API 操作 RecordScreen，包含列舉／選擇視窗、截圖、錄影、MCP client 設定與 token 保護。Codex 在本專案工作時可使用 `$recordscreen-control` 呼叫此 skill。

Release 使用 size optimization、LTO、單一 codegen unit、移除符號與 abort panic；這些設定會讓 release 編譯較久，但不影響 debug build。MCP JSON 範例中的 `YOUR_USER` 要換成目前 Windows 使用者資料夾名稱。

## 螢幕與隱私

截圖和錄影會包含目前桌面上可見的內容；開始錄影前請確認畫面適合保存。檔案只寫入設定的本機輸出資料夾。此版本不錄製系統／麥克風音訊，也不提供遠端控制或網路監聽。
