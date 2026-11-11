# LumaNote

LumaNote 是 Windows 11 桌面 AI 個人助理。桌面上常駐一個小精靈，可以把事情交給它處理：整理筆記成待辦、產生文件、設定提醒。模型供應商可替換，任務、工具、授權、記憶與排程由本機程式管理。

技術組成：Tauri 2 外殼、React + TypeScript + Vite 前端、Rust 核心、SQLite。

## 目前狀態（2026/11/12）

這是開發中的原型，**桌面程式裡看到的對話與任務仍是示範用的假資料**，尚未接上真實模型。

| 部分 | 狀態 |
| --- | --- |
| Rust 核心 `lumanote-core`：任務狀態機、協調器、工具、授權、排程器、兩個模型協定適配器 | 已實作，321 項自動測試通過；尚未用真實 API 驗證 |
| 桌面外殼：浮動精靈、主面板、設定視窗、系統匣、輪盤、拖曳與位置記憶 | 已在 Windows 11 實機建置並執行 |
| 前端介面：對話、任務、今日、授權卡、設定、六款可切換造型（含 2.5D） | 已實作，接的是假後端 |
| 核心接上外殼（讓對話與任務變成真的） | **未開始，這是下一步** |
| 系統通知、安裝包、活動觀察、主動建議、網路研究 | 未開始 |

完整的現況、已驗證與未驗證的項目、已知問題與接手後的工作順序，見 [docs/HANDOFF.zh-TW.md](docs/HANDOFF.zh-TW.md)。

## 在 Windows 上執行

需要：Microsoft C++ Build Tools（「使用 C++ 的桌面開發」工作負載）、Rust 1.90 以上（MSVC 工具鏈）、Node.js 22.12 以上（或 20.19 以上）。Windows 11 已內建 WebView2。

```powershell
npm install
npm run tauri dev
```

第一次編譯需要數分鐘。安裝步驟、成功時應看到的畫面與常見錯誤，見 [docs/WINDOWS_SETUP.zh-TW.md](docs/WINDOWS_SETUP.zh-TW.md)。

## 常用指令

| 指令 | 用途 |
| --- | --- |
| `npm run tauri dev` | 啟動桌面程式（開發模式） |
| `npm run dev` | 只啟動前端，在瀏覽器以假後端預覽（http://127.0.0.1:5173） |
| `npm run build` | 型別檢查並建置前端 |
| `cargo test -p lumanote-core` | 核心測試 |
| `cargo test -p lumanote-shell` | 外殼的幾何與位置保存測試 |
| `cargo clippy --workspace --all-targets -- -D warnings` | Rust 靜態檢查 |
| `npm run sync:skins` | 從 `docs/design/skins` 驗證並同步造型素材到前端 |
| `npm run test:layered` | 分層造型素材驗證器的測試 |

## 目錄

```text
crates/lumanote-core/   Rust 核心，不依賴 Tauri 與 Windows，可在任何平台測試
src-tauri/         Tauri 外殼：視窗、系統匣、浮動視窗行為
src/               前端
scripts/           造型素材同步與驗證
tests-ui/          Playwright 介面檢查
docs/              文件，從 docs/README.md 開始
```

## 文件

| 文件 | 內容 |
| --- | --- |
| [docs/HANDOFF.zh-TW.md](docs/HANDOFF.zh-TW.md) | 交接：現況、驗證範圍、已知問題、下一步 |
| [docs/DEVELOPMENT.zh-TW.md](docs/DEVELOPMENT.zh-TW.md) | 開發指南：架構導覽、測試、如何新增工具、適配器、造型 |
| [docs/TECHNICAL_HANDBOOK.zh-TW.md](docs/TECHNICAL_HANDBOOK.zh-TW.md) | 技術手冊：產品設計、架構、契約、安全規則、驗收標準 |
| [docs/README.md](docs/README.md) | 所有文件的索引 |

## 授權

尚未選定授權條款。在選定之前，預設保留所有權利。
