# RecordScreen

Windows 10+ 桌面錄影與截圖程式，使用 Rust 建置，FFmpeg 負責擷取畫面與輸出 MP4／PNG。桌面 GUI 啟動時會同時提供只監聽本機的 agent API；另提供 MCP stdio 模式，方便 Codex、Claude Desktop 等 MCP client 將錄影／截圖動作註冊為工具。

## 使用需求

- Windows 10 或更新版本。
- v0.1.7 起的 x64 安裝包將 Visual C++ 執行階段靜態連結至 RecordScreen，不需另外安裝 VC++ Redistributable。舊版若顯示找不到 `VCRUNTIME140.dll`，請使用 v0.1.7 或更新的安裝包升級。
- FFmpeg 需包含 Windows `gdigrab` 輸入裝置與 `libx264` 編碼器。Windows 安裝程式會先檢查 PATH 與常見安裝位置；找不到時預設提供下載並安裝 FFmpeg。
- 第一次啟動後，API 會監聽 `127.0.0.1:17321`；桌面 app 必須保持開啟，agent 才能呼叫。

預設輸出位置：`%USERPROFILE%\Videos\RecordScreen`。可在 GUI 修改輸出資料夾。錄影預設 30 FPS，可在 GUI 調整 10–60 FPS。修改擷取設定後按「Apply settings／套用設定」才會儲存並套用；介面語系可先預覽，不影響錄影按鈕。介面字型預設使用 Windows 已安裝的「Microsoft JhengHei UI（微軟正黑體）」，不需另外安裝字型。

在「Settings／設定」頁可切換 English／繁體中文。首次啟動預設 English。語系、輸出資料夾、FPS 和錄影倒數秒數儲存於 `%LOCALAPPDATA%\RecordScreen\setting.json`；若檔案不存在，啟動時自動建立。舊版設定檔未含倒數欄位時維持立即開始錄影；從更早版本升級且尚無 JSON 時，會從 `language.txt` 匯入語系。按「Apply settings／套用設定」後，下次啟動沿用。設定檔無法讀取時會顯示錯誤，保留原檔供修復。

主介面為緊湊視窗（預設 380×600，最小 340×500），底部分頁列以圖示切換擷取、檔案庫、設定與 Agent 四頁。擷取頁以大型計時器與圓形錄影／停止按鈕為中心，另有截圖與開啟輸出資料夾的圓形按鈕，並可用來源選單選擇整個桌面或指定視窗。倒數秒數（0／3／5／10 秒）與語系等設定於設定頁調整。`Ctrl+R` 可在 RecordScreen 視窗有焦點時開始／停止錄影，`Ctrl+Shift+S` 可截圖。設定頁可用 Windows 原生資料夾選擇器選取輸出位置，底部另有 About 區塊。檔案庫列出輸出資料夾中最近 30 個 RecordScreen 產出檔，可在檔案總管定位。Agent API 的 Token 預設隱藏，只有按「Show／顯示」才會出現在畫面上，也可按「Copy token／複製 Token」。

主介面標題會顯示目前 app 版本，方便確認安裝版本與回報問題。程式視窗、執行檔與安裝捷徑使用相同的 RecordScreen 圖示；圖示原始檔位於 `assets/recordscreen.ico`。

可用 `Tab` 和 `Shift+Tab` 移動到具有焦點外框的控制項，再用 `Enter` 或 `Space` 操作按鈕或切換頁籤。

系統匣：設定頁的「Minimize to tray／最小化至系統匣」預設開啟（儲存於 `setting.json` 的 `minimize_to_tray`）。開啟時，最小化視窗或按標題列的系統匣按鈕會將視窗隱藏至系統匣；左鍵點擊系統匣圖示即可還原視窗。系統匣選單提供「Show RecordScreen」、開始／停止錄影、截圖與結束。直接關閉視窗（X）會結束程式，若正在錄影會先停止錄影。

v0.1.6 改善設定儲存與 Agent 連線診斷：設定會先寫入同目錄暫存檔並同步，再替換 `setting.json`；寫入或替換失敗時保留原設定。「Discard changes／捨棄變更」可恢復目前已套用的設定。Agent API 頁會顯示啟動、監聽或失敗狀態；連接埠占用或 Token 無法讀寫時會顯示原因。指定視窗以 HWND 與 PID 判斷，切換文件或瀏覽器分頁造成標題改變時仍可擷取；視窗關閉、最小化或已屬於其他程序時須重新選取。

「錄影來源」可選整個桌面或指定 app 視窗。清單會顯示標題與 PID；同一 app 的不同視窗可分別選取。指定後，截圖及錄影都只擷取該視窗，即使它被其他視窗遮住也不會改錄整個桌面。目標 app 必須保持開啟且不要最小化；錄影期間不能切換來源。

## 建置與啟動

在 Windows x64 安裝 Rust stable（MSVC 工具鏈）、Visual Studio Build Tools 的「使用 C++ 的桌面開發」工作負載，以及 FFmpeg。在專案根目錄執行下列命令。

```powershell
cargo build --release --locked
.\scripts\check-runtime-dependencies.ps1
.\target\release\recordscreen.exe
```

`.cargo/config.toml` 設定 `+crt-static`，讓 x64 執行檔可在沒有 VC++ Redistributable 的電腦上啟動。請保留該設定，避免以 `RUSTFLAGS` 覆蓋。DLL 檢查腳本使用 Visual Studio 的 `dumpbin.exe` 驗證連結結果。

FFmpeg Windows build 可使用 [gyan.dev builds](https://www.gyan.dev/ffmpeg/builds/)。請將 `bin` 資料夾加入使用者 `PATH`，或複製 `ffmpeg.exe` 至 `recordscreen.exe` 同一目錄。

### 修改與驗證

- 桌面畫面與互動位於 `ui/app.slint` 和 `src/ui.rs`。擷取流程位於 `src/main.rs` 和 `src/windows_capture.rs`。HTTP 與 MCP 共用 `src/server.rs`。
- 提交前執行 `cargo fmt --all -- --check` 和 `cargo test --locked`。Windows PR 工作流程會再執行這兩項檢查、編譯 release 執行檔及安裝包，並附上安裝包的 SHA-256。
- 若需要從乾淨狀態重新建置，可在專案根目錄執行 `cargo clean`。Cargo 產物位於 `target/`，歷次安裝包集中於 `dist/`。

## Windows 安裝程式

從 GitHub Releases 下載 `RecordScreen-Setup-<版本>-x64.exe`，執行後預設安裝至目前使用者的 `%LOCALAPPDATA%\Programs\RecordScreen`，不需要系統管理員權限。開始功能表會建立捷徑；安裝精靈也可選擇建立桌面捷徑。解除安裝可從 Windows「已安裝的應用程式」或開始功能表執行。

安裝程式會先尋找 PATH、WinGet、Scoop、Chocolatey 與常見 FFmpeg 目錄。若找不到，安裝精靈會預設勾選下載 FFmpeg 9.0.2 essentials（約 35 MB）；即使偵測到其他 FFmpeg，仍可手動勾選此項以修復。檔案會從 [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) 以 HTTPS 下載，並在解壓前驗證固定的 SHA-256。FFmpeg 會放在 RecordScreen 安裝目錄的 `_ffmpeg_runtime`，不會修改系統 PATH；解除安裝 RecordScreen 時會一併移除這個目錄。若電腦目前離線，可取消勾選並稍後再安裝 FFmpeg。此下載由 Gyan.dev 提供，該版本依 GPLv3 發佈；本安裝程式不重新散佈 FFmpeg 二進位檔。

無人值守安裝若需強制下載 FFmpeg，請執行對應版本的安裝程式並加上 `/VERYSILENT /DOWNLOADFFMPEG`。

自行從原始碼啟動時，仍須先安裝支援 `gdigrab` 與 `libx264` 的 FFmpeg，並放入 PATH 或程式同目錄。設定 MCP 時，server command 預設使用 `%LOCALAPPDATA%\Programs\RecordScreen\recordscreen.exe`。JSON 設定通常不會展開環境變數，請填入實際完整路徑。可用 PowerShell 取得：

```powershell
Join-Path $env:LOCALAPPDATA 'Programs\RecordScreen\recordscreen.exe'
```

本機打包時，請先完成上方的 release 建置與 DLL 檢查，再用 Inno Setup 7 的 `ISCC.exe` 編譯 `installer\RecordScreen.iss`。Inno Setup 會從已建置的執行檔讀取版本並產生 `dist\RecordScreen-Setup-<版本>-x64.exe`。也可手動執行 GitHub Actions 的 **Windows installer** 工作流程，從該次執行下載安裝包與雜湊。正式發布使用與 Cargo 版本相符的 `v<版本>` 標籤。

## Agent HTTP API

Agent 只應連線至 `http://127.0.0.1:17321`。GUI 的 Agent API 頁可按「Show」查看 bearer token；token 也儲存在 `%LOCALAPPDATA%\RecordScreen\agent-token.txt`。請勿把 token 貼到遠端服務、提交到 Git，或提供給不受信任的 agent。每個控制端點都需要 `Authorization: Bearer <token>`。API 會拒絕非 loopback Host／Origin。

| Method | Path | 功能 |
| --- | --- | --- |
| GET | `/health` | 無認證健康狀態 |
| GET | `/api/v1/status` | 讀取錄影狀態、輸出位置、app `version` 與 `process_id` |
| GET | `/api/v1/windows` | 列出目前可選取的可見 app 視窗與 HWND |
| POST | `/api/v1/target` | 必須提供 `hwnd`。數字指定視窗，明確傳 `null` 切回整個桌面 |
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

先開啟 RecordScreen GUI。在 Codex CLI 或桌面的 `config.toml` 加入下列設定，將 `command` 換成已安裝執行檔的完整路徑，然後重新載入 Codex。

```toml
[mcp_servers.recordscreen]
command = 'C:\Users\YOUR_USER\AppData\Local\Programs\RecordScreen\recordscreen.exe'
args = ['--mcp']
```

先用 `get_status` 確認 GUI 在線及目前的擷取來源，再用 `list_windows` 取得 `select_window` 需要的 `hwnd`。若要切回整個桌面，須明確送出 `{"hwnd": null}`。開始擷取前先確認來源與使用者授權，再呼叫 `take_screenshot` 或 `start_recording`。`start_recording` 的 `fps` 可省略，提供時須為 10–60 的整數。錄影完成後用 `stop_recording` 取得檔案路徑。

stdio 子程序從使用者資料夾讀取 token，透過本機 API 呼叫 GUI。若無法連線，請先開啟 GUI，在 Agent API 頁確認監聽狀態。若 MCP client 採用 JSON 格式，請把同一個 `command` 和 `args` 放在 `mcpServers.recordscreen`，並依 JSON 規則跳脫路徑中的反斜線。

MCP 2025-06-18 或 2025-11-25 的用戶端可讀取成功結果的 `structuredContent`。所有支援版本也可讀取 `content` 內的 JSON 文字。收到逾時時先查 `get_status`，確認錄影狀態後再決定是否重試。

## Codex Agent skill

專案包含 `.agents/skills/recordscreen-control/SKILL.md`，說明如何透過 MCP 或本機 HTTP API 操作 RecordScreen，包含列舉／選擇視窗、截圖、錄影、MCP client 設定與 token 保護。Codex 在本專案工作時可使用 `$recordscreen-control` 呼叫此 skill。

## 螢幕與隱私

截圖和錄影會包含目前桌面上可見的內容；開始錄影前請確認畫面適合保存。檔案只寫入設定的本機輸出資料夾。此版本不錄製系統／麥克風音訊，也不提供遠端控制或網路監聽。

## 授權

RecordScreen 的圖形介面以 [Slint](https://slint.dev) 1.18.1 建置，依 Slint Royalty-free Desktop License 使用。依該授權條款要求，設定頁的 About 區塊以 AboutSlint 顯示 Slint 的授權與致謝資訊，請勿移除。
