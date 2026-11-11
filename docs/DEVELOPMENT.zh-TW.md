# 開發指南

給要動手改程式的人。現況與待辦見 [HANDOFF](HANDOFF.zh-TW.md)，設計規則見 [TECHNICAL_HANDBOOK](TECHNICAL_HANDBOOK.zh-TW.md)。

## 環境

| 工具 | 版本 | 備註 |
| --- | --- | --- |
| Rust | 1.90 以上，Windows 上用 MSVC 工具鏈 | `rustup default stable-msvc` |
| Node.js | 22.12 以上，或 20.19 以上 | 低於此版本前端工具會無法啟動 |
| Microsoft C++ Build Tools | 「使用 C++ 的桌面開發」工作負載 | 只有 Windows 需要 |

版本鎖定 Tauri 2.x；3.0 已有 alpha，不採用。確切版本見 `src-tauri/Cargo.toml` 與 `package.json`。

核心 `elf-core` 可以在 Linux、macOS、Windows 上編譯與測試。桌面外殼的目標平台只有 Windows；在 Linux 上可以編譯檢查（需要 WebKitGTK 開發套件），但 Windows 行為必須在 Windows 上驗證。

## 架構導覽

```text
前端 (src)  ──IPC──>  外殼 (src-tauri)  ──呼叫──>  核心 (crates/elf-core)
                                                    │
                                                    ├─ agent      協調器：模型循環與工具執行
                                                    ├─ model      統一模型契約與各協定適配器
                                                    ├─ tools      工具登錄表與工具
                                                    ├─ policy     授權與路徑檢查
                                                    ├─ scheduler  排程
                                                    ├─ storage    SQLite 與 migration
                                                    ├─ task       任務狀態機
                                                    └─ ports      對外能力的 trait
```

目前外殼到核心的呼叫尚未實作（見 HANDOFF）。

三條貫穿全專案的原則，改程式前請先理解：

1. **核心不知道平台。** 時間、通知、憑證、檔案系統、HTTP 都透過 `ports.rs` 的 trait 注入。所有時間相關邏輯走 `Clock`，測試用假時鐘，不做真實等待。
2. **模型的輸出不可信。** 模型只能「提出」工具呼叫；驗證、授權、執行、判定完成都由核心負責。模型的文字不能把任務標為完成。
3. **沒有證據不算完成。** 任務完成需要成果檔或成功的寫入步驟。外部寫入結果不明時標為「結果不確定」，不自動重試。

## 測試

```bash
cargo test -p elf-core                                   # 核心
cargo test -p elf-shell                                  # 外殼的幾何與位置保存
cargo clippy --workspace --all-targets -- -D warnings    # 靜態檢查
npm run build                                            # 前端型別檢查與建置
npm run test:layered                                     # 分層素材驗證器
```

介面檢查（需要自行安裝 Playwright）：

```bash
npm run build
npx vite preview --port 4173 &
node tests-ui/capture.cjs http://localhost:4173
```

它會以假後端跑完展示情境、檢查輪盤與造型行為，並把截圖存到 `tests-ui/screenshots/`（不納入版本控制）。

品質門檻：測試與 clippy 全過；測試以外不用 `unwrap`、`expect`；不用 `unsafe`；公開項目有文件註解。回報「完成」要附實際執行的指令與結果。

## 常見工作

### 新增一個工具

1. 在 `crates/elf-core/src/tools/` 新增檔案，實作 `Tool` trait：名稱、給模型看的說明、參數的 JSON Schema、需要的授權動作、**動作類別**（`action_class`，必填；沒有明確歸類的一律視為外部寫入）、副作用類別、逾時、輸出上限、`execute`。
2. 工具結果要標明信任等級。模型讀到的任何非使用者親手輸入的內容都是不受信任的，包含本機檔案。
3. 寫入類工具要回傳證據（成果檔或可驗證的結果），並且能在當機後重跑而不重複執行。
4. 對外傳送、發布、付款、刪除類的工具，每次都需要使用者確認，不接受常設授權；這由政策引擎依動作類別強制，工具本身不需要也不應該繞過。
5. 在登錄表註冊，並補測試：參數驗證、授權、逾時、輸出截斷、提示注入情境。

規則出處：手冊第 8 章與 8.1。

### 新增一個模型協定

1. 在 `crates/elf-core/src/model/adapters/` 新增適配器，實作 `ModelAdapter`。請求組裝、串流轉譯、結束原因對應、用量對應、錯誤對應都在這裡；工具呼叫的片段組裝使用共用的 `ToolCallAssembler`，SSE 解析使用 `model/sse.rs`。
2. 不直接使用 HTTP 客戶端，透過 `HttpTransport`。金鑰在送出請求時才從 `CredentialStore` 讀取，只送往設定檔的主機，不得出現在錯誤、日誌、事件或除錯輸出中。
3. 準備串流樣本放進 `crates/elf-core/fixtures/<協定>/`，用 `ReplayTransport` 以多種切塊大小回放測試。至少涵蓋：純文字、分片的工具呼叫、平行工具呼叫、串流中途錯誤、限流、斷線、格式錯誤的參數、取消。
4. 加一個協調器層級的測試，用新適配器跑通「筆記 → 待辦 → 文件 → 完成」，確認核心邏輯不需修改。

契約見手冊 7.1。供應商的不透明狀態（例如思考區塊簽章）只回傳給同一供應商；切換供應商只能在一輪工具呼叫完整結束之後。

### 修改資料庫結構

1. 在 `crates/elf-core/migrations/` 新增下一個編號的 SQL 檔。**不要修改已存在的 migration。**
2. 在 `src/storage/migrate.rs` 的 `MIGRATIONS` 加一筆。
3. 每個 migration 在單一交易內執行，失敗時資料庫不變。補一個測試：用有真實資料列的舊版資料庫升級，確認沒有遺失。

### 新增或更新造型

1. 把素材放進 `docs/design/skins/<ID>/`：六種狀態的 SVG、`collapsed.svg`、`avatar.svg`、`skin.json`；分層版放在 `layered/` 子資料夾（`layers.json`、`states.json`、各圖層的 `@1x.png` 與 `@2x.png`）。格式見 `docs/briefs/` 的後兩份簡報。
2. 執行 `npm run sync:skins`。驗證失敗的造型不會被複製；分層素材失敗時該造型退回平面版。
3. 新造型要在 `src/skins/registry.ts` 的 `SKINS` 加一行。
4. `src/assets/skins/` 是同步產生的結果，請不要手動修改，改來源後重新同步。

### 調整浮動視窗或輪盤的尺寸

尺寸在兩處各定義一次：`src/features/floating/layout.ts`（前端）與 `src-tauri/src/geometry.rs`（外殼），加上 `src-tauri/tauri.conf.json` 的視窗初始大小。三者必須一致，由 `cargo test -p elf-shell` 的 `constants_match_frontend` 與建置前的 `npm run check:floating` 檢查。改尺寸時若會影響已保存的位置，要處理 `geometry.rs` 中的位置版本換算（`SAVED_POSITION_VERSION`）。

### 新增一個 IPC 命令

現階段：

1. 型別加到 `src/lib/ipc-types.ts`（唯一宣告 IPC 型別的地方）。
2. 在 `src/lib/ipc-mock.ts` 實作假行為。
3. 需要真實外殼行為時，在 `src-tauri` 加 `#[tauri::command]`，於 `build.rs` 註冊，並在對應視窗的 capability 檔加上權限；再把命令名稱加到 `src/lib/ipc-tauri.ts` 的 `NATIVE_COMMANDS`。
4. 敏感命令必須在 Rust 端再驗證一次呼叫來源與參數，不能只靠 capability。

核心接上外殼後，型別會改由 Rust 產生，這個流程會簡化。

## 前端慣例

- 顏色、字體、圓角、間距只定義在 `src/styles/tokens.css`，元件不寫死數值。
- 介面文字集中在 `src/lib/strings.ts`，使用繁體中文。
- 前端重新連線或掛載時先向後端讀取權威狀態，不以事件推算任務是否完成。
- 模型輸出的 Markdown 不允許原始 HTML；外部連結一律經 `openExternal`。
- 造型以 `<img>` 呈現，不把 SVG 內嵌進頁面。
- 動畫只用位移、旋轉、縮放、透明度；尊重系統的「減少動態」設定。
- 浮動視窗不取得鍵盤焦點，所以其中的互動只能依賴滑鼠。

## 在 Windows 上要特別注意的事

- `data-tauri-drag-region` 只有在被按下的元素本身帶有該屬性時才生效。浮動精靈是按鈕，所以拖曳由 `startWindowDrag` 明確啟動。
- 系統通知在開發模式下的顯示來源不正確，驗收要用安裝版。
- Windows 的現代待命會暫停桌面程式，睡眠期間到期的提醒只能在喚醒後依補送政策處理。
- 隱藏的視窗不會自動釋放 WebView2 記憶體。
- 其他已知的 Tauri 在 Windows 上的問題，見 `docs/research/R1-tauri-windows.zh-TW.md`。

## 文件維護

- 設計或規則改變時改手冊，並在第 16 章加一筆修訂紀錄。
- 實作進度、驗證範圍、已知問題改 HANDOFF。
- 查證資料要附來源與查證日期，區分已查證事實與推論。
