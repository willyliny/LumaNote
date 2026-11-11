# 文件索引

第一次接手請依序讀：`HANDOFF` → `DEVELOPMENT` → `TECHNICAL_HANDBOOK` 的第 3、7、8 章。

## 主要文件

| 文件 | 何時讀 |
| --- | --- |
| [HANDOFF.zh-TW.md](HANDOFF.zh-TW.md) | 想知道現在做到哪、什麼驗證過、接下來做什麼 |
| [DEVELOPMENT.zh-TW.md](DEVELOPMENT.zh-TW.md) | 要動手改程式 |
| [TECHNICAL_HANDBOOK.zh-TW.md](TECHNICAL_HANDBOOK.zh-TW.md) | 想知道某個行為「應該」是什麼。設計、契約與規則以此為準 |
| [WINDOWS_SETUP.zh-TW.md](WINDOWS_SETUP.zh-TW.md) | 第一次在 Windows 上建置，或建置出錯 |
| [WEEK_PLAN.zh-TW.md](WEEK_PLAN.zh-TW.md) | 一週衝刺的工作包、分工與每日進度 |

## 參考資料

| 位置 | 內容 | 注意 |
| --- | --- | --- |
| [research/R1-tauri-windows.zh-TW.md](research/R1-tauri-windows.zh-TW.md) | Tauri 2 與 Windows 的版本、外掛、已知問題查證 | 查證日 2026/11/11；Windows 行為多數未實測 |
| [research/R2-model-protocols.zh-TW.md](research/R2-model-protocols.zh-TW.md) | Anthropic Messages、OpenAI Chat Completions 等協定的線路格式 | 依官方文件，未呼叫真實 API；報告內標有未驗證項目 |
| `research/fixtures/` | 串流樣本原稿 | 測試實際使用的是 `crates/elf-core/fixtures/` 的複本，修改時兩邊都要改 |
| [security/SECURITY_REVIEW.zh-TW.md](security/SECURITY_REVIEW.zh-TW.md) | 獨立安全審查的 16 項發現 | 針對較早的程式碼；全部已修正，處理結果見 HANDOFF 與手冊 8.1 |
| `briefs/` | 給視覺設計的三份簡報 | 素材格式契約以最後兩份為準 |
| `archive/` | 手冊 0.2 原稿 | 僅供對照 |

## 設計素材（docs/design）

| 位置 | 內容 |
| --- | --- |
| `design/skins/<造型 ID>/` | 六款造型的平面 SVG 與 `skin.json`；前端素材的來源 |
| `design/skins/<造型 ID>/layered/` | 2.5D 分層 PNG、`layers.json`、`states.json` |
| `design/skins/night-voyage-tokens.json` | 夜航系列的介面配色 |
| `design/design-tokens.json` | 預設（quiet-desk）系列的介面配色與尺寸 |
| `design/skins/*-DELIVERY.zh-TW.md` | 設計方的交付說明 |
| `design/characters/`、`design/icons/`、`design/cyber-penpal/` | 第一輪的概念與素材，已被 `skins/` 取代，僅供參考 |

前端不直接讀 `docs/design`。素材經 `npm run sync:skins` 驗證後複製到 `src/assets/skins/`，兩邊都在版本控制內。
