# R1 研究報告：Tauri 2 × Windows 11 技術查核

- 工作包：R1（研究）
- 查核日期：**2026-11-11**（以下所有「已查核」項目皆為此日期）
- 對應手冊：`docs/TECHNICAL_HANDBOOK.zh-TW.md` §3、§3.1、§4、§5、§10、§11、§12
- 證據來源：crates.io API、npm registry、官方文件（v2.tauri.app、learn.microsoft.com、sqlite.org）、以及在對應 release tag 上直接讀取的原始碼。**沒有任何版本號或 API 形狀取自記憶。**
- 標記方式：
  - **【事實】**：有來源可查的內容，來源可能是文件、registry 或原始碼。
  - **【推論】**：由事實推導而來，尚未在 Windows 實機驗證。
  - **【未能查證】**：明確寫出缺口。

---

## 0 決策總表

| # | 項目 | 建議 | 信心 | 主要風險 |
| --- | --- | --- | --- | --- |
| 1 | Tauri 版本 | `tauri = "2.12.5"`、`tauri-build = "2.7.4"`、CLI `@tauri-apps/cli 2.12.4`、`@tauri-apps/api 2.12.4`；Rust **≥ 1.90**（MSVC toolchain） | 高 | Tauri **3.0.0-alpha.6** 已發布，`dev` 分支已是 v3。本週一律鎖 2.x，不要執行 `cargo update` 跨到 3.x 預發版 |
| 2 | 系統匣 | 用 Tauri **核心功能**，不是外掛：`tauri` 的 `tray-icon` feature，並在 Rust 端用 `TrayIconBuilder` 建立 | 高 | 低 |
| 2 | 通知 | `tauri-plugin-notification 2.8.1`。只用來「顯示」通知，排程一律放在 Rust（elf-core scheduler） | 中 | dev 模式通知顯示為 PowerShell；只有用 NSIS/MSI **安裝後**才會帶正確的 AUMID。桌面版會忽略排程參數。按鈕點擊只有在 toast 還在畫面上時才會回報 |
| 2 | 其他外掛 | single-instance 2.5.5、autostart 2.7.0、global-shortcut 2.4.1、window-state 2.5.0、opener 2.7.3；updater 2.14.1 本週不啟用；**不要用 shell 的 open（已 deprecated）** | 高 | single-instance 必須第一個註冊 |
| 3 | 浮動視窗 | `focusable(false)` + `focused(false)` + `always_on_top(true)` + `transparent(true)` + `decorations(false)` + `shadow(false)` + `skip_taskbar(true)` + `no_redirection_bitmap(true)`，建立時先隱藏，載入完成再顯示 | 中低 | 拖曳時焦點閃動（#10767）；`skipTaskbar` 偶發失效（#10422）；透明視窗白閃；不同 DPI 的多螢幕定位（#6843，仍 open） |
| 3 | 視窗權限 | 每個視窗一個 capability 檔（`floating` / `main` / `settings`），不要用 `core:default`；App 命令用 `AppManifest::commands` 納入 ACL | 高 | 一個視窗符合多個 capability 時，權限會合併 |
| 4 | 憑證 | `keyring-core 1.0.1` + `windows-native-keyring-store 1.1.0`（或 `keyring 4.2.0` 預設 `v1`），只放在 `src-tauri`、`cfg(windows)` 底下。elf-core 只定義 `CredentialStore` trait | 高 | Credential Manager 單筆上限 **2560 bytes**，較長的 OAuth token 可能放不下；**stronghold 不建議**（上游 2024-05 後就沒有發版） |
| 5 | SQLite | `rusqlite = { version = "=0.41.1", features = ["bundled"] }` + `rusqlite_migration = "2.7.0"`，啟用 WAL | 高 | **rusqlite 0.42.0 是今天才發布的，rusqlite_migration 2.7.0 只支援 `^0.41`**，絕對不能升到 0.42 |
| 6 | TS 型別產生 | `tauri-specta = "=2.0.0-rc.28"` + `specta = "=2.0.0-rc.28"` + `specta-typescript = "=0.0.15"`。elf-core 的型別只 derive `specta::Type`，specta 不依賴 Tauri | 中 | 仍是 RC，rc.25 到 rc.28 在幾週內就連續發版，必須用 `=` 鎖版；i64/u64 預設禁止匯出 |
| 7 | 活動採樣 | `windows = "0.62.2"`（與 Tauri 2.12.5 依賴的 0.62 同一版號系列），feature 清單見 §7 | 高 | 前景視窗可能是 NULL；UWP 會顯示為 ApplicationFrameHost；Modern Standby 期間桌面程式會被暫停 |
| 8 | 背景常駐 | `CloseRequested` 時改為 `prevent_close` + `hide`，`RunEvent::ExitRequested { code: None }` 時 `prevent_exit`；系統匣的「結束」呼叫 `app.exit(0)` | 高 | 隱藏視窗的 WebView2 **不會自動暫停或降低記憶體**；長時間隱藏時要自行呼叫 `SetMemoryUsageTargetLevel(Low)`，或直接銷毀視窗 |

---

## 1 Tauri 版本與 Windows 開發環境

### 1.1 版本（crates.io / npm，查核日 2026-11-11）

| 套件 | 最新穩定版 | 發布日期 | MSRV | 備註 |
| --- | --- | --- | --- | --- |
| `tauri`（crate） | **2.12.5** | 2026-10-21 | 1.90 | 同時有 `3.0.0-alpha.6`（2026-11-06） |
| `tauri-build` | **2.7.4** | 2026-10-21 | 1.90 | |
| `tauri-cli`（crate） | **2.12.4** | 2026-10-21 | 1.90 | |
| `@tauri-apps/cli`（npm） | **2.12.4** | 2026-10-21 | – | `next` tag = 3.0.0-alpha.6 |
| `@tauri-apps/api`（npm） | **2.12.4** | 2026-10-14 | – | `next` tag = 3.0.0-alpha.3 |
| `create-tauri-app`（npm） | 4.7.5 | – | – | |
| `vite`（npm，參考用） | 8.3.8 | – | – | engines：`node ^20.19.0 \|\| >=22.12.0` |

【事實】在 tag `tauri-v2.12.5` 上，workspace `Cargo.toml` 寫的是 `rust-version = "1.90"`，各官方外掛的 2.x 版本也都標 1.90。

【事實】Tauri 主 repo 的預設分支 `dev` 已經在開發 v3。v2 的程式碼要看 tag `tauri-v2.12.5`（另有 `2.11` 分支）。本報告引用的 Tauri 原始碼一律以 `tauri-v2.12.5` tag 為準。

【推論】Cargo 的 `^2` 不會自動選到 `3.0.0-alpha`，因為 semver 預發版不參與比對，所以使用 `"2.12"` 是安全的。但 npm 用 `@next` 或 `npm i @tauri-apps/api@latest` 以外的指令時要小心。建議 `package.json` 寫 `"~2.12.4"`。

【推論，未在原始碼確認】tauri-cli 會比對 Rust `tauri` 與 npm `@tauri-apps/api` 的 major.minor，兩邊建議都維持在 2.12。

可貼用的 `Cargo.toml` 片段（src-tauri）：

```toml
[build-dependencies]
tauri-build = { version = "2.7.4", features = [] }

[dependencies]
tauri = { version = "2.12.5", features = ["tray-icon"] }   # tray-icon = 核心系統匣
```

可貼用的 `package.json` 片段：

```json
"dependencies":   { "@tauri-apps/api": "~2.12.4" },
"devDependencies":{ "@tauri-apps/cli": "~2.12.4" }
```

### 1.2 全新 Windows 11 安裝步驟

【事實，來源：Tauri Prerequisites】

1. **Microsoft C++ Build Tools**：下載 Visual Studio Build Tools，安裝時勾選 **「Desktop development with C++」** workload。
2. **WebView2**：Windows 10 1803 以後及 Windows 11 已內建，可以略過。否則從 WebView2 Runtime 下載頁取得 **Evergreen Bootstrapper**。
3. **Rust**：用 rustup 安裝，default host triple 必須是 MSVC（`x86_64-pc-windows-msvc`；ARM 機器用 `aarch64-pc-windows-msvc`）。已安裝的話執行 `rustup default stable-msvc`。安裝完重開終端機，必要時重開機。
4. **Node.js**：前端框架需要，安裝 LTS 後確認 `node -v`、`npm -v`。可選擇執行 `corepack enable`。Vite 8 需要 Node `^20.19.0 || >=22.12.0`（npm engines 欄位）。
5. **VBSCRIPT**（只有產生 MSI 時需要）：設定 → 應用程式 → 選用功能 → 更多 Windows 功能 → 勾選 VBSCRIPT。缺少時會出現 `failed to run light.exe`。官方註明 VBSCRIPT 正在被 deprecate。

執行方式：

- `npm install` → `npm run tauri dev`
- `npm run tauri build`：預設會產生 NSIS（`-setup.exe`）與 MSI。

【事實】打包工具會自動下載。`tauri-bundler`（tag `tauri-v2.12.5`）的 `NSIS_URL` 指向 `nsis-3.13.zip`，`WIX_URL` 指向 `wix314-binaries.zip`（WiX v3.14），第一次 build 需要能連到 GitHub。

【事實】NSIS 預設是 **per-user** 安裝（`%LOCALAPPDATA%`，不需要系統管理員權限）。WebView2 安裝模式預設為 `downloadBootstrapper`（0 MB）；`embedBootstrapper` 約增加 1.8 MB，`offlineInstaller` 約 127 MB，`fixedVersion` 約 180 MB。

【建議】本週只產生 **NSIS**（`bundle.targets: ["nsis"]`），可以避開 VBSCRIPT/WiX 的問題。通知測試也一定要用 NSIS 安裝版，見 §2.2。

【未能查證】目前 Rust stable 與 Node LTS 的確切版號：static.rust-lang.org 與 nodejs.org 被代理擋下（403），只確認了 MSRV 1.90 與 Vite 的 engines 需求。

**來源**

- https://v2.tauri.app/start/prerequisites/
- https://v2.tauri.app/distribute/windows-installer/
- https://crates.io/crates/tauri
- https://www.npmjs.com/package/@tauri-apps/api
- https://github.com/tauri-apps/tauri/tree/tauri-v2.12.5 （`Cargo.toml`、`crates/tauri-bundler/src/bundle/windows/{nsis,msi}/mod.rs`）

---

## 2 官方外掛：版本、權限、通知行為

### 2.1 版本與權限識別碼（crates.io / npm / plugins-workspace `v2` 分支，HEAD 2026-11-10）

| 功能 | 形式 | Rust crate | npm | 預設權限集內容（`<plugin>:default`） |
| --- | --- | --- | --- | --- |
| 系統匣 | **核心**（`tauri` feature `tray-icon`） | – | `@tauri-apps/api/tray` | `core:tray:default` = allow-new、get-by-id、remove-by-id、set-icon、set-menu、set-tooltip、set-title、set-visible…（**會允許前端建立 tray**） |
| 通知 | 外掛 | `tauri-plugin-notification = "2.8.1"` | `@tauri-apps/plugin-notification 2.8.1` | `notification:default` 共 17 項：is-permission-granted、request-permission、notify、register-action-types、register-listener、remove-listener、cancel、get-pending、remove-active、get-active、check-permissions、show、batch、list/create/delete-channel、permission-state |
| 全域快捷鍵 | 外掛 | `tauri-plugin-global-shortcut = "2.4.1"` | `@tauri-apps/plugin-global-shortcut 2.4.1` | **預設什麼都不開**。可用的有 `global-shortcut:allow-register`、`allow-register-all`、`allow-unregister`、`allow-unregister-all`、`allow-is-registered` |
| 開機啟動 | 外掛 | `tauri-plugin-autostart = "2.7.0"` | `@tauri-apps/plugin-autostart 2.7.0` | allow-enable、allow-disable、allow-is-enabled |
| 單一實例 | 外掛（**只有 Rust**，npm 上沒有對應套件） | `tauri-plugin-single-instance = "2.5.5"` | – | – |
| 視窗狀態 | 外掛 | `tauri-plugin-window-state = "2.5.0"` | `@tauri-apps/plugin-window-state 2.5.0` | allow-filename、allow-restore-state、allow-save-window-state |
| 更新 | 外掛 | `tauri-plugin-updater = "2.14.1"` | `@tauri-apps/plugin-updater 2.14.1` | allow-check、allow-download、allow-install、allow-download-and-install |
| 開啟外部連結 | 外掛 | `tauri-plugin-opener = "2.7.3"` | `@tauri-apps/plugin-opener 2.7.3` | allow-open-url、allow-reveal-item-in-dir、allow-default-urls（mailto/tel/http/https） |
| shell | 外掛 | `tauri-plugin-shell = "2.6.0"` | `@tauri-apps/plugin-shell 2.6.0` | allow-open。**Rust 原始碼標註 `#[deprecated(since = "2.1.0", note = "Use tauri-plugin-opener instead.")]`** |
| 定位（選用） | 外掛 | `tauri-plugin-positioner = "2.4.0"` | `@tauri-apps/plugin-positioner 2.4.0` | – |

【事實】所有外掛的 2.x 都另有 `3.0.0-alpha.x` 版本，不要混用。

【事實】單一實例：README 寫明「plugins run in the order they were added … make sure that this plugin is registered first」。Windows 實作使用 `CreateMutexW` 與 `WM_COPYDATA` 轉送第二個實例的 argv。

【事實】開機啟動：底層用 `auto-launch 0.6.0`，在 Windows 寫入 `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run`，並讀取 `...\Explorer\StartupApproved\Run`（使用者從工作管理員停用時會反映在這裡）。

【事實】全域快捷鍵：底層用 `global-hotkey`，Windows 呼叫 `RegisterHotKey`。在 `global-hotkey` master 的原始碼中，`ERROR_HOTKEY_ALREADY_REGISTERED` 會轉成 `Error::AlreadyRegistered`。【推論】外掛使用的 0.8 系列行為應該相同，所以手冊 §4 的「快捷鍵衝突時顯示替代選擇」可以靠這個錯誤實作。若改由其他程式先搶到熱鍵，沒有事件可以通知。

【事實】視窗狀態外掛儲存的是**實體座標**（`PhysicalPosition` / `PhysicalSize`）。還原時只有在儲存的矩形與某個現有螢幕相交時才會 `set_position`，否則交給 OS 決定位置。

【建議】主面板與設定視窗可以直接使用此外掛。浮動視窗另外寫一份持久化（見 §3），以符合手冊「邏輯座標保存」的要求。

### 2.2 通知在 Windows 上的行為

【事實，官方文件平台表】Windows 一欄寫著：*"Only works for installed apps. Shows powershell name & icon in development."*

【事實，原始碼 `plugins/notification/src/desktop.rs`】

- Windows 上，只有在**執行檔所在目錄不是以 `\target\debug` 或 `\target\release` 結尾**時，才會設定 `app_id(identifier)`，也就是 AUMID 等於 `tauri.conf.json` 的 `identifier`。換句話說：
  - `tauri dev`，或直接執行 `target\release\*.exe`，都不設定 AUMID，因此顯示 PowerShell 的名稱與圖示。
  - 如果自訂了 `CARGO_TARGET_DIR` / `build.target-dir`，路徑判斷會失準，AUMID 會被設定，但沒有對應的 Start Menu 捷徑。官方修正分支 `fix/notification/installdetection` 停在 2024-06，**至今沒有合併**。【推論】這種情況下 toast 很可能完全不會出現（依 Microsoft 文件，沒有捷徑就「cannot raise a toast」）。
- 【事實】在 tag `tauri-v2.12.5` 的打包器中，NSIS 安裝程式對 Start Menu 與桌面捷徑呼叫 `SetLnkAppUserModelId`，MSI 的 `main.wxs` 設定 `System.AppUserModel.ID = {{bundle_id}}`。所以**用安裝程式安裝後**，AUMID 與捷徑會一致。
- 【事實，Microsoft 文件】桌面（未封裝）程式必須在 Start Menu 有帶 `System.AppUserModel.ID` 的捷徑，否則「you cannot raise a toast notification from a desktop app」。
- 【事實】桌面版的 `show()` 只會使用 title、body、icon、sound，以及 action type 中的按鈕。原始碼註解寫明：*"the scheduling and grouping options are ignored"*。**也就是說，外掛在桌面上沒有排程能力**，這和手冊 §3 / §11「排程器不依賴 WebView 計時器」的方向一致，必須由 Rust scheduler 在到期時呼叫 `show()`。
- 【事實】動作按鈕：2.6.0 起桌面版支援 action（changelog：*"Report actions on desktop … reaches `onAction` and the new Rust `Notification::on_action`"*）。限制如下：
  - 只有在已經註冊 Rust `on_action` handler 或 JS `onAction` listener 時，按鈕才會出現。
  - Windows 上 *"a click is only reported while the toast is on screen, not once it moved to the Action Center"*。
  - `remove_active` 在 Windows 沒有作用。
- 【事實】桌面版的 `request_permission` / `permission_state` 永遠回傳 `Granted`。2.5.0 修正了 Windows 上 `isPermissionGranted()` 永遠是 false 的 bug，所以至少要用 2.5.0 以上。
- 【事實】`actions` 與 `channels` 的官方文件段落仍寫著「Actions API is only available on mobile」，與 2.6.0 之後的原始碼不一致。**以原始碼為準。**

【推論】對提醒功能的影響：

1. 主視窗隱藏時，只要 Rust 程序還活著，`app.notification().builder()...show()` 就能從 Rust 直接發出，不需要 WebView。在 `on_action` 註冊 Rust handler，即可處理「稍後提醒」「完成」等按鈕。
2. 使用者若讓 toast 進入通知中心後才點擊，app 收不到事件。因此提醒的權威狀態要放在 DB，下次開啟面板時再顯示「未回應」，不能依賴點擊事件。
3. **驗收與手測都必須用 NSIS 安裝版。** 開發者 agent 在 `tauri dev` 下看到 PowerShell 圖示是正常現象，不是 bug。
4. 使用者若在 Windows 設定中關閉此 app 的通知，或開啟專注模式／勿擾，toast 不會出現，而外掛也無法偵測。【未能查證】是否有 API 可以查詢 Windows 的通知停用狀態。

**來源**

- https://v2.tauri.app/plugin/notification/
- https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/notification/src/desktop.rs
- https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/notification/CHANGELOG.md
- https://github.com/tauri-apps/plugins-workspace/tree/fix/notification/installdetection
- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
- https://learn.microsoft.com/en-us/windows/win32/shell/enable-desktop-toast-with-appusermodelid
- https://v2.tauri.app/learn/system-tray/
- 各外掛 `permissions/autogenerated/reference.md`（plugins-workspace `v2`）
- https://github.com/tauri-apps/global-hotkey/blob/dev/src/platform_impl/windows/mod.rs

---

## 3 多視窗：浮動入口、主面板、設定

### 3.1 可用的 API（tag `tauri-v2.12.5`，`WebviewWindowBuilder` 與 `tauri.conf.json` 的 `app.windows[]`）

【事實】以下方法與設定欄位都存在：

- `focused` / `focus`
- `focusable`
- `transparent`
- `decorations`
- `always_on_top`
- `skip_taskbar`
- `shadow`
- `visible`
- `no_redirection_bitmap`（**Windows only**，設定 `WS_EX_NOREDIRECTIONBITMAP`，用來避免透明視窗白閃）
- `prevent_overflow`
- `visible_on_all_workspaces`
- `drag_and_drop` / `disable_drag_drop_handler`
- `background_color`

【事實】`focus` 與 `focusable` 在 config 裡預設都是 `true`。

【事實，tao 0.37.2 原始碼】`focusable(false)` 在 Windows 會加上 `WS_EX_NOACTIVATE`。以 `focused(false)` 建立的視窗，第一次顯示時用 `SW_SHOWNOACTIVATE`。

【事實】`shadow` 的文件說明：Windows 上 `true` 會讓無邊框視窗出現 1px 白邊（Win11 會有圓角），`false` 對有邊框的視窗沒有作用。

建議的浮動視窗設定【推論，需要實機驗證】：

```jsonc
// tauri.conf.json > app.windows[]
{ "label": "floating", "url": "/floating", "width": 72, "height": 72,
  "decorations": false, "transparent": true, "shadow": false,
  "alwaysOnTop": true, "skipTaskbar": true, "resizable": false,
  "focus": false, "focusable": false, "visible": false,
  "noRedirectionBitmap": true }
```

- 主面板：`label: "main"`，一般有邊框視窗，`visible: false`，由系統匣、快捷鍵或浮動入口開啟。
- 設定視窗：`label: "settings"`，**需要時才建立，關閉時銷毀**（見 §8 的記憶體說明）。

### 3.2 已知的 Windows 問題（GitHub issues，查核日 2026-11-11）

| 問題 | 狀態 | 對本案的意義 |
| --- | --- | --- |
| #14386 `focusable(false)` 後收不到鍵盤事件 | open。維護者表示受限於 WebView2，*"we're blocked by webview2 here"* | 浮動入口**不能放輸入框**。點擊後應呼叫 Rust 命令開啟 `main` 並由 `main` 取得焦點。這正好符合手冊「不搶焦點」的要求 |
| #10767 拖曳區域造成焦點來回切換，且吞掉 mouseup | Win10/Win11 都有回報，2.10.3 仍可重現，沒有修正 PR | 拖曳浮動入口時可能先失焦再取得焦點，**可能會短暫搶走焦點**。【推論】若無法接受，可考慮用 Rust 端的 `start_dragging`，或改用 `app-region: drag` CSS（官方文件提到此 CSS 適合簡單的拖曳列，也支援觸控與筆）。兩種都需要實機比較 |
| #10422 `skipTaskbar` 在 Windows 不穩定 | open。維護者無法重現。社群用 `WS_EX_TOOLWINDOW` 的作法可靠，但官方不打算內建 | 【推論】如果實測仍出現在工作列，就在 src-tauri 用 `windows` crate 自行加上 `WS_EX_TOOLWINDOW`。這樣也會讓視窗不出現在 Alt+Tab，對浮動入口反而合適 |
| #14831、#14515 透明視窗建立或顯示時白閃 | #15410 加入了 `no_redirection_bitmap`，**tag 2.12.5 原始碼中已有此 API**。#14515 另有社群作法：先移到螢幕外再 hide | 使用 `noRedirectionBitmap: true`，建立時 `visible:false`，前端 `DOMContentLoaded` 後再請 Rust `show()` |
| #6843 不同縮放比例的多螢幕上，視窗尺寸錯誤 | open，沒有確認的 workaround | 浮動入口固定尺寸很小，影響有限。但位置還原要自己做邊界檢查 |
| #14088 所有視窗隱藏約 50 分鐘後當機 | closed。修正在 `tray-icon ≥ 0.21.2` | 【事實】Tauri 2.12.5 依賴 `tray-icon 0.25`，已包含修正 |

### 3.3 多螢幕與 DPI

- 【事實】window-state 外掛以實體座標儲存，並在還原時檢查螢幕是否相交（§2.1）。
- 【推論，建議做法】浮動入口自行儲存 `{ monitor_name, logical_x, logical_y, scale_factor_at_save }`。啟動時用 `available_monitors()` 找出同名螢幕，再以該螢幕的 `scale_factor` 換算。找不到就放在主螢幕右下角的工作區內。這樣符合手冊 §4「邏輯座標保存＋啟動時檢查邊界」。

### 3.4 Capability 每個視窗分開設定

【事實，官方文件與 `tauri-utils` 原始碼】

- 檔案放在 `src-tauri/capabilities/*.json`（或 `.toml`）。資料夾內的檔案**預設全部自動啟用**；一旦在 `tauri.conf.json > app.security.capabilities` 中明確列出，就**只有列出的會生效**。
- 欄位：
  - `identifier`
  - `description`
  - `windows`：支援 glob
  - `webviews`：支援 glob
  - `permissions`
  - `platforms`
  - `remote.urls`
  - `local`：預設 true
- 一個視窗符合多個 capability 時，權限會**合併**，官方安全提示也特別提醒這一點。
- `core:window:default` 只包含**唯讀**類權限（get/is/scale-factor/monitor 等）。拖曳、顯示、隱藏要另外加上 `core:window:allow-start-dragging`、`allow-show`、`allow-hide`、`allow-set-focus` 等。
- **App 自訂命令**：預設所有在 `invoke_handler` 註冊的命令，**任何視窗都能呼叫**。要納入 ACL，必須在 `build.rs` 使用：

  ```rust
  tauri_build::try_build(tauri_build::Attributes::new()
      .app_manifest(tauri_build::AppManifest::new().commands(&["submit_task", "open_panel" /* … */])))
  ```

  設定後會自動產生 `allow-<command>` / `deny-<command>`。也可以在 `src-tauri/permissions/**` 自行定義權限集。
- 【推論，需編譯確認】app 權限在 capability 裡**不加前綴**，例如寫 `"allow-submit-task"`。這是依據 `tauri-build/src/acl.rs` 中 `APP_ACL_KEY` 的處理推得，建議第一次 `cargo build` 後查看 `gen/schemas/desktop-schema.json` 確認。

建議的權限分配【推論】：

| 檔案 | `windows` | 權限 |
| --- | --- | --- |
| `capabilities/floating.json` | `["floating"]` | `core:event:allow-listen`、`core:event:allow-unlisten`、`core:window:allow-start-dragging`、`allow-open-panel`、`allow-get-status` |
| `capabilities/main.json` | `["main"]` | `core:event:default`、`core:window:allow-hide`、`allow-submit-task`、`allow-cancel-task`、`allow-respond-to-approval`…、`opener:allow-open-url`（限定 https） |
| `capabilities/settings.json` | `["settings"]` | 偏好設定、觀察政策、記憶管理、`allow-set-api-key`（**只寫入，不提供讀取**）、`autostart:default`、`global-shortcut:allow-register`… |

**不要用 `core:default`**，因為它包含 `core:tray:default`（允許前端建立或修改 tray）與 `core:event:allow-emit`。

**來源**

- https://v2.tauri.app/security/capabilities/
- https://v2.tauri.app/learn/window-customization/
- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/webview/webview_window.rs
- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-utils/src/acl/capability.rs
- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-build/src/acl.rs
- https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/windows/window_state.rs
- issues：
  - https://github.com/tauri-apps/tauri/issues/14386
  - https://github.com/tauri-apps/tauri/issues/10767
  - https://github.com/tauri-apps/tauri/issues/10422
  - https://github.com/tauri-apps/tauri/issues/14831
  - https://github.com/tauri-apps/tauri/issues/14515
  - https://github.com/tauri-apps/tauri/pull/15410
  - https://github.com/tauri-apps/tauri/issues/6843
  - https://github.com/tauri-apps/tauri/issues/14088

---

## 4 憑證儲存（API key 不進前端）

### 4.1 選項比較

| 選項 | 版本與日期 | 狀態 | 評估 |
| --- | --- | --- | --- |
| `keyring`（4.x） | **4.2.0**（2026-08-29），MSRV 1.88 | 活躍。repo 已移到 `open-source-cooperative/keyring-rs` | 4.x 改為生態系結構：`keyring` 只是包裝層，預設 `v1` feature 保留舊 API，在 Windows 使用 Credential Manager |
| `keyring-core` + `windows-native-keyring-store` | **1.0.1**（2026-10-13）+ **1.1.0**（2026-05-24），MSRV 1.88 | 活躍 | 官方 README 建議新專案直接依賴 `keyring-core` 與需要的 store，依賴最小。Windows store 只依賴 `windows-sys 0.61.2` |
| `tauri-plugin-stronghold` | 2.4.0（2026-09-26） | 外掛仍在發版，但上游 `iota_stronghold` 最後一版是 **2.1.0（2024-05-13）** | 需要自行管理密碼或 argon2 salt 檔；設計目標是讓前端存取 vault，與「金鑰永不進前端」的方向相反。**不建議** |
| plugins-workspace 分支 `plugin/secure-storage` | 未發布 | – | 【未能查證】內容與時程；本週不可依賴 |
| `tauri-plugin-keyring`（第三方） | 0.1.0（2024-12-23） | 久未更新 | 不建議 |

【事實】`windows-native-keyring-store` README 的重要警告：*"Windows Credential Manager loses updates when credentials … are written and deleted concurrently."* 該 crate 用以使用者 SID 命名的 mutex 序列化自己的呼叫。

【事實，Microsoft 文件】`CREDENTIAL.CredentialBlobSize` 上限為 `CRED_MAX_CREDENTIAL_BLOB_SIZE` = **5×512 = 2560 bytes**。該 crate 超過時會回傳 `Error::TooLong`。

【未能查證，本次只確認到上限】Credential Manager 的加密機制（一般認為是 DPAPI，與使用者登入綁定）。`CREDENTIALW` 文件頁沒有寫到加密方式。

### 4.2 建議

- **elf-core**：只定義 `CredentialStore` trait，例如 `put(ref) / get(ref) / delete(ref)`，DB 中只存 `credential_ref`。不依賴任何 keyring crate，Linux CI 用記憶體 fake。
- **src-tauri**：

  ```toml
  [target.'cfg(windows)'.dependencies]
  keyring-core = "1.0.1"
  windows-native-keyring-store = { version = "1.1.0", default-features = false }  # 關掉 search 以免引入 regex
  ```

  啟動時呼叫一次 `keyring_core::set_default_store(windows_native_keyring_store::Store::new()?)`，之後用 `keyring_core::Entry::new("com.<identifier>.elf", "<provider>:<profile>")` 的 `set_password` / `get_password` / `delete_credential`。這是官方 example 的寫法。

  如果偏好單一依賴，也可以用 `keyring = "4.2.0"`（預設 `v1`），API 為 `keyring::Entry::new(service, user)`。但在 Linux 上它會引入 zbus secret-service，所以同樣只放在 `cfg(windows)` 底下。
- **IPC 介面設計**：
  - 只提供 `set_api_key(provider, key)`（只寫入）、`has_api_key(provider) -> bool`、`delete_api_key(provider)`。
  - **不提供任何回傳金鑰的命令。**
  - `set_api_key` 只授權給 `settings` 視窗。
  - 金鑰只在 Rust 的 `HttpTransport` 組 header 時讀取，日誌以 `secrecy` 或自訂 `Debug` 遮蔽。
- 【推論】2560 bytes 足夠存放各家 API key。OAuth access token（JWT）有機會超過，因此 Credential Manager 只存 refresh token；若仍超過，可分段存放，或用 DPAPI（`CryptProtectData`）加密後存檔。**這一點手冊 §10 沒有提到。**

**來源**

- https://crates.io/crates/keyring
- https://github.com/open-source-cooperative/keyring-rs
- https://github.com/open-source-cooperative/windows-native-keyring-store
- https://github.com/open-source-cooperative/keyring-core
- https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw
- https://v2.tauri.app/plugin/stronghold/
- https://crates.io/crates/iota_stronghold

---

## 5 SQLite（elf-core，不依賴 Tauri）

### 5.1 選項比較（查核日 2026-11-11）

| 選項 | 版本 | 評估 |
| --- | --- | --- |
| `rusqlite` + `bundled` | 0.42.0（**2026-11-11 當天發布**）、0.41.1（2026-10-28），MSRV 1.89 | 同步 API，最輕量。`bundled` 自帶 SQLite：0.41.1（libsqlite3-sys 0.39.0）為 **SQLite 3.54.0**，0.42.0（libsqlite3-sys 0.40.0）為 3.54.1。Windows 不需要另外安裝 SQLite |
| `sqlx`（sqlite） | 0.9.0（2026-05-21），MSRV 1.94 | async；編譯期 `query!` 需要 `DATABASE_URL` 或 offline 檔，增加 CI 設定成本；連線池對單機單寫入者用處不大 |
| `tauri-plugin-sql` | 2.6.0 | **依賴 `tauri ^2.12` 與 `sqlx ^0.8`**，違反 §3.1「elf-core 不得相依 Tauri」，而且設計上把 SQL 開放給前端。**排除** |

遷移工具：

| 工具 | 版本 | 評估 |
| --- | --- | --- |
| `rusqlite_migration` | **2.7.0**（2026-11-10），MSRV 1.89 | 依賴 **`rusqlite ^0.41.0`**。用 `PRAGMA user_version` 記錄版本，不建額外表。`from-directory` feature 可用 `include_dir!` 讀 `migrations/` 資料夾。README 範例中就有啟用 WAL |
| `refinery` | 0.10.0（2026-10-02） | 可行，但用額外表記錄，較重 |

**關鍵版本陷阱【事實】**：`rusqlite_migration 2.7.0` 的依賴是 `rusqlite ^0.41.0`，Cargo 不會把 0.42 視為相容。若 elf-core 寫 `rusqlite = "0.42"`，就會同時編譯兩份 libsqlite3-sys，並因 `links = "sqlite3"` 衝突而**無法建置**。

### 5.2 建議（可貼用）

```toml
# crates/elf-core/Cargo.toml
rusqlite = { version = "=0.41.1", features = ["bundled"] }
rusqlite_migration = { version = "2.7.0", features = ["from-directory"] }
include_dir = "0.7"
```

連線初始化【推論，依 sqlite.org 文件整理】：

```text
PRAGMA journal_mode = WAL;   -- 持久化，設一次即可
PRAGMA synchronous = NORMAL; -- WAL 下常見設定
PRAGMA foreign_keys = ON;    -- 預設 OFF，每條連線都要設
PRAGMA busy_timeout = 5000;
```

建議只開**一條寫入連線**（放在單一 DB 執行緒或 `Mutex<Connection>` 中），讀取可以另開連線。Tauri 命令以 `spawn_blocking`，或透過 channel 交給 DB 執行緒處理。

### 5.3 WAL 注意事項

【事實，sqlite.org】

- WAL 模式會持久化。
- 會產生 `-wal` 與 `-shm` 檔，WAL 是資料庫持久狀態的一部分，**不可以單獨刪除或分開搬移**。
- 只能在同一台主機上使用，不能放在網路磁碟。
- 只允許一個寫入者。
- 自動 checkpoint 在 WAL 達到 1000 頁（約 4 MB）時觸發；長時間存在的讀取者會讓 WAL 無限成長。
- `secure_delete` 預設為 OFF；FAST 模式仍會在 freelist 頁留下痕跡。
- 3.51.3 以後修正了「WAL-reset bug」，本案使用的 3.54.x 已包含此修正。

【推論，對應手冊 §9「資料清除要考慮 WAL」】使用者要求刪除資料時依序執行：

1. `PRAGMA secure_delete = ON`（或在建立連線時就開啟）
2. `DELETE`
3. `PRAGMA wal_checkpoint(TRUNCATE)`
4. 視需要執行 `VACUUM`

備份要用 `rusqlite` 的 backup API 或 `VACUUM INTO`，不要直接複製 `.db` 檔案。

DB 位置建議放在 `app_local_data_dir()`，避免 OneDrive 同步的資料夾。【推論】這是依據 WAL 不能用於網路檔案系統推得，OneDrive 同步的行為本次未查證。

**來源**

- https://crates.io/crates/rusqlite
- https://github.com/rusqlite/rusqlite （`libsqlite3-sys/sqlite3/sqlite3.h` 的 tag v0.41.1 與 master）
- https://github.com/cljoly/rusqlite_migration
- https://crates.io/crates/sqlx
- https://crates.io/crates/tauri-plugin-sql
- https://sqlite.org/wal.html
- https://sqlite.org/pragma.html

---

## 6 IPC 型別產生（Rust 為唯一來源）

| 工具 | 版本與日期 | Tauri 2 相容 | 維護狀態 | 能力 |
| --- | --- | --- | --- | --- |
| `tauri-specta` | **2.0.0-rc.28（2026-11-11 當天）**；穩定版 1.0.2 僅適用 Tauri v1 | 是（依賴 `tauri ^2`，並開啟 tauri 的 `specta` feature） | 活躍，最近 commit 是 2026-11-11 | 同時產生**命令包裝函式與事件型別**；支援 `Channel<T>`；`ErrorHandlingMode` 可以選 Throw、Result、DataError |
| `specta` / `specta-typescript` | =2.0.0-rc.28（2026-11-05）/ 0.0.15 | – | 活躍 | **不依賴 Tauri**，elf-core 可以 derive `specta::Type`；有 chrono、uuid、jiff、time、serde_json 等 feature |
| `ts-rs` | **12.0.1**（2026-01-31），MSRV 1.78 | 與 Tauri 無關 | 活躍，最近 commit 是 2026-11-09 | 只產生型別，**不產生命令包裝**，`invoke` 名稱與參數需要手寫或另外處理 |
| `taurpc` | 0.8.2（2026-07-03） | 【未能查證】細節 | – | 另一套 RPC 風格，本週不建議引入 |

【事實】tauri-specta 文件寫明：*"Tauri Specta v2 is still in beta … it is really important you use `=` before your Specta version"*。

【事實】`specta-typescript` 預設**禁止匯出 BigInt 類型**（i64、u64、i128 等），會回傳 `BigIntForbidden` 錯誤。

建議（可貼用）：

```toml
# crates/elf-core/Cargo.toml（不依賴 Tauri）
specta = { version = "=2.0.0-rc.28", features = ["derive", "chrono"] }  # 依實際時間型別選 chrono / jiff / time

# src-tauri/Cargo.toml
tauri = { version = "2.12.5", features = ["tray-icon", "specta"] }
specta = { version = "=2.0.0-rc.28", features = ["derive"] }
specta-typescript = "=0.0.15"
tauri-specta = { version = "=2.0.0-rc.28", features = ["derive", "typescript"] }
```

用法【事實，tauri-specta 文件】：

```rust
let builder = tauri_specta::Builder::<tauri::Wry>::new()
    .commands(tauri_specta::collect_commands![submit_task, cancel_task])
    .events(tauri_specta::collect_events![TaskProgress]);
#[cfg(debug_assertions)]
builder.export(specta_typescript::Typescript::default(), "../src/lib/bindings.ts")?;
tauri::Builder::default()
    .invoke_handler(builder.invoke_handler())
    .setup(move |app| { builder.mount_events(app); Ok(()) })
```

命令需要同時標 `#[tauri::command]` 與 `#[specta::specta]`。

【推論】設計建議：

- 對 IPC 的 ID、`sequence`、時間戳，一律用字串（ULID/UUID、RFC 3339）或 `u32`/`f64`，避開 BigInt 的限制。
- 在 CI（可以在 Linux 執行）加上「產生 bindings 後與 commit 版本 diff 不一致即失敗」的檢查。
- 若 tauri-specta 的 RC 出問題，備案是 **ts-rs 12.0.1 只產生型別，加上手寫一層薄的 `ipc.ts`**。

**來源**

- https://crates.io/crates/tauri-specta
- https://github.com/specta-rs/tauri-specta （`src/lib.rs` 文件）
- https://github.com/specta-rs/specta （`specta-typescript/src/error.rs`）
- https://crates.io/crates/ts-rs
- https://github.com/Aleph-Alpha/ts-rs

---

## 7 Windows 活動採樣（`windows` crate）

### 7.1 版本與 feature（已解開 `windows-0.62.2.crate` 原始碼核對）

- 【事實】`windows` 最新版為 **0.62.2**（2025-10-06），MSRV 1.82。Tauri 2.12.5 與 tao 0.37 也使用 `windows 0.62`，同一版號系列可以避免重複編譯。
- `windows-sys` 0.61.2 是較輕量的替代選擇，keyring store 使用的就是它。

```toml
# src-tauri/Cargo.toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.62.2", features = [
  "Win32_Foundation",                 # HWND, HANDLE, CloseHandle
  "Win32_UI_WindowsAndMessaging",     # GetForegroundWindow, GetWindowThreadProcessId, GetWindowTextW, EnumChildWindows,
                                      # WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK/UNLOCK, WM_POWERBROADCAST, PBT_*, DEVICE_NOTIFY_WINDOW_HANDLE
  "Win32_System_Threading",           # OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION
  "Win32_UI_Input_KeyboardAndMouse",  # GetLastInputInfo, LASTINPUTINFO
  "Win32_System_RemoteDesktop",       # WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION
  "Win32_System_Power",               # RegisterSuspendResumeNotification, RegisterPowerSettingNotification
  "Win32_System_SystemServices",      # GUID_CONSOLE_DISPLAY_STATE
  "Win32_System_SystemInformation",   # GetTickCount64
] }
```

【事實，原始碼簽章】

- `GetWindowThreadProcessId(hwnd, Option<*mut u32>) -> u32`
- `OpenProcess(...) -> windows_core::Result<HANDLE>`
- `QueryFullProcessImageNameW(...) -> Result<()>`
- `WTSRegisterSessionNotification(hwnd, u32) -> Result<()>`
- `RegisterSuspendResumeNotification(HANDLE, REGISTER_NOTIFICATION_FLAGS) -> Result<HPOWERNOTIFY>`：本身在 `Win32_System_Power` 底下，但需要同時開啟 `Win32_UI_WindowsAndMessaging` feature。

常數值：

| 常數 | 值 |
| --- | --- |
| `WM_WTSSESSION_CHANGE` | 689 |
| `WTS_SESSION_LOCK` | 7 |
| `WTS_SESSION_UNLOCK` | 8 |
| `WM_POWERBROADCAST` | 536 |
| `PBT_APMSUSPEND` | 4 |
| `PBT_APMRESUMEAUTOMATIC` | 18 |

### 7.2 已知陷阱

| 陷阱 | 依據 | 處理 |
| --- | --- | --- |
| `GetForegroundWindow` 回傳 NULL | 【事實，MS】*"can be NULL in certain circumstances, such as when a window is losing activation"* | 記為「未知／過渡」樣本，不切斷區段，也不寫入 |
| 對高權限或受保護程序 `OpenProcess` 失敗 | 【事實，MS】受保護程序不能授予 `PROCESS_QUERY_INFORMATION`，但 `PROCESS_QUERY_LIMITED_INFORMATION` 不在禁止清單中 | 一律使用 `PROCESS_QUERY_LIMITED_INFORMATION` + `QueryFullProcessImageNameW`。失敗時記為 `unknown(pid)`。【未能查證】一般權限程序對以系統管理員身分執行的程式（高 integrity）是否一定能取得 limited 權限，MS 該頁沒有說明，需要實機測試 |
| UWP 或商店應用顯示為 `ApplicationFrameHost.exe` | 【事實，社群 Stack Overflow】`GetForegroundWindow` + `GetWindowThreadProcessId` 取得的是宿主程序 | 【推論，社群常見作法，未經官方文件背書】`EnumChildWindows` 找出 PID 與宿主不同的子視窗（通常 class 為 `Windows.UI.Core.CoreWindow`），取其 PID。找不到就保留 `ApplicationFrameHost` 並加上視窗標題（須依標題授權設定） |
| `GetLastInputInfo` 只涵蓋目前 session | 【事實，MS】*"provides session-specific user input information"*；tick 值*"not guaranteed to be incremental"* | 用 `GetTickCount()` 搭配 `wrapping_sub` 計算，結果異常時夾到 0。`dwTime` 是 32 位元【推論：約 49.7 天繞回一次，MS 該頁未提】 |
| 需要「視窗」才能收到 lock/unlock | 【事實，MS】`WTSRegisterSessionNotification` 需要 hWnd，銷毀前必須呼叫 `WTSUnRegisterSessionNotification`；過早呼叫可能回傳 `RPC_S_INVALID_BINDING`（需等待 `Global\TermSrvReadyEvent`） | 【推論】在 src-tauri 建立一個專用的隱藏 message 視窗執行緒（`CreateWindowExW` + `HWND_MESSAGE` + 自己的 message loop）處理 WTS 與 power 訊息，不要掛在 Tauri 視窗上，因為 Tauri 視窗可能被銷毀。開機自動啟動時遇到 `RPC_S_INVALID_BINDING` 要重試。【未能查證】MS 文件沒有說明 message-only 視窗能否收到 WTS 通知；若實測收不到，改用隱藏的頂層視窗 |
| 睡眠與恢復 | 【事實，MS】`RegisterSuspendResumeNotification`（Win8+）可以用 `DEVICE_NOTIFY_WINDOW_HANDLE` 或 `DEVICE_NOTIFY_CALLBACK`。恢復時一定會送 `PBT_APMRESUMEAUTOMATIC`，由使用者觸發時另外再送 `PBT_APMRESUMESUSPEND` | 使用 callback 形式即可，不需要視窗 |
| **Modern Standby** | 【事實，MS】DAM 階段會*"suspends desktop applications"*，之後*"Windows prevents desktop applications from running during any part of modern standby"* | 【推論】睡眠期間 Rust 計時器不會觸發，**提醒會在喚醒後才補送**，必須走手冊 §11 的補送流程。螢幕關閉可用 `RegisterPowerSettingNotification(GUID_CONSOLE_DISPLAY_STATE)` 偵測。【未能查證】進入 Modern Standby 時是否一定會收到 `PBT_APMSUSPEND`，所以每次採樣都要用「實際經過時間是否遠大於 5 秒」作為缺口偵測的保險 |

**來源**

- https://crates.io/crates/windows
- https://github.com/microsoft/windows-rs
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getforegroundwindow
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getlastinputinfo
- https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsregistersessionnotification
- https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registersuspendresumenotification
- https://learn.microsoft.com/en-us/windows/win32/power/wm-powerbroadcast
- https://learn.microsoft.com/en-us/windows/win32/procthread/process-security-and-access-rights
- https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/prepare-software-for-modern-standby
- https://kiwix.lema.org/stackoverflow.com_en_all_2023-11/a/32513438 （Stack Overflow 存檔，ApplicationFrameHost 問題描述）

---

## 8 背景常駐與 WebView2 記憶體

### 8.1 視窗全部關閉後讓程序繼續存活

【事實，tag `tauri-v2.12.5` 原始碼】

- `tauri-runtime-wry` 在最後一個視窗被移除時，會發出 `RunEvent::ExitRequested { code: None, .. }`。
- 呼叫 `api.prevent_exit()` 可以阻止程序結束，但若是 `restart()` 觸發的則會被忽略。
- `code` 的意義：
  - `None`：使用者操作導致，例如最後一個視窗被關閉。
  - `Some(_)`：程式主動呼叫 `AppHandle::exit()` 或 `restart()`。

建議作法（社群在 #13511 的共識，加上上述原始碼的語意）：

```rust
.on_window_event(|w, e| if let tauri::WindowEvent::CloseRequested { api, .. } = e {
    if w.label() == "main" { api.prevent_close(); let _ = w.hide(); }
})
// ...
.build(ctx)?.run(|_app, e| if let tauri::RunEvent::ExitRequested { code: None, api, .. } = e {
    api.prevent_exit();   // 只擋「最後一個視窗被關閉」這種情況
});
// 系統匣「結束」→ app.exit(0) → code = Some(0)，不會被擋
```

【事實，#13511】社群回報，只靠 `code` 判斷時，在 Windows 上仍可能擋住「正常的程序終止」。【推論】登出或關機（`WM_QUERYENDSESSION`）的處理需要實機驗證；手冊要求的系統匣「結束」選項必須存在。

### 8.2 WebView2 記憶體：隱藏與銷毀

【事實】

- Tauri 的 `hide()` 只呼叫 tao 的 `window.set_visible(false)`。wry 只在 `webview.set_visible` 時呼叫 `controller.SetIsVisible`，並沒有把視窗隱藏同步給 WebView2。
- wry 0.57 有 `set_memory_usage_level`（`ICoreWebView2_19::SetMemoryUsageTargetLevel`），但 **Tauri 2.12.5 沒有包裝這個 API**。需要時可以透過 `webview.with_webview(|pw| pw.controller())` 取得 `ICoreWebView2Controller`，再用 `webview2-com 0.39` 自行呼叫。

【事實，Microsoft 文件】

- `MemoryUsageTargetLevel = LOW`：*"Scripts will not be impacted and continue to run"*，部分記憶體可能被換出到磁碟，不需要先設 `IsVisible=false`，也**不會自動恢復為 NORMAL**。
- `TrySuspend`：必須先 `IsVisible=false`，會*"pauses WebView script timers and animations"*。WebView 變為可見時會自動恢復。
- 兩種作法*"not advisable to mix"*。
- 文件**沒有說** `IsVisible=false` 會自動暫停。

【推論】對本案的建議：

1. 浮動入口常駐，不需要處理，本身就很小。
2. 主面板隱藏時保留視窗，讓開啟速度快。若隱藏超過 N 分鐘，就設定 `MemoryUsageTargetLevel=Low`；`show()` 前再設回 Normal。**不要用 `TrySuspend`**，因為事件監聽可能被暫停，而任務進度本來就要在重連時從 Rust 讀取權威狀態。
3. 設定視窗關閉即**銷毀**。
4. 所有提醒、排程與採樣都在 Rust，WebView 被暫停或銷毀都不影響。

【未能查證】實際記憶體數字。本環境沒有 Windows 機器，所以隱藏前後的 private bytes、每個視窗額外 renderer 的成本都需要第一天在實機上用工作管理員量測。

**來源**

- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/app.rs
- https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-runtime-wry/src/lib.rs
- https://github.com/tauri-apps/wry/blob/dev/src/webview2/mod.rs
- https://github.com/tauri-apps/tauri/issues/13511
- https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_19
- https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2_3

---

## 9 與手冊的差異或缺口

1. **§4「多螢幕位置用邏輯座標保存」**：官方 window-state 外掛儲存的是實體座標。浮動入口需要自行實作持久化（§3.3），或接受外掛的作法並修改手冊用語。
2. **§10「API key 與 OAuth token 存於作業系統憑證儲存」**：Windows Credential Manager 單筆上限 2560 bytes，大型 OAuth token 可能放不下，需要補一條策略（§4.2）。
3. **§11 背景生命週期**：手冊只寫到「完整退出或關機時無法提醒」，沒有提到 **Modern Standby 睡眠期間桌面程式會被暫停**，提醒只能在喚醒後補送。建議補上。
4. **§13、§14 測試**：通知只在**安裝版**正確顯示（dev 模式顯示 PowerShell），所以提醒相關的驗收必須用 NSIS 安裝版，週計畫要預留打包時間。
5. **§3.1 elf-core 相依限制**：採用 tauri-specta 時，elf-core 需要依賴 `specta`（不是 Tauri），符合字面規定，但要在手冊中明確允許。
6. **§12 前端命令**：建議新增 `set_api_key` / `has_api_key` / `delete_api_key`（只寫入，不可讀出），以及 `open_panel`（給浮動入口用）。

## 10 未能查證的項目一覽

- 目前 Rust stable 與 Node LTS 的確切版號（官方站點被代理擋下）。
- 執行 `tauri-cli` 時 npm 與 crate 版本比對的確切規則（沒有讀到 CLI 原始碼）。
- 實際 Windows 行為（本環境不是 Windows）：
  - `focusable(false)` 與拖曳搭配時是否仍會搶焦點
  - `skipTaskbar` 的穩定性
  - 透明視窗白閃是否已消除
  - WebView2 記憶體數字
  - message-only 視窗能否收到 WTS 通知
  - 對系統管理員權限程序取得 limited 權限
  - Modern Standby 是否送出 `PBT_APMSUSPEND`
  - 自訂 target-dir 時通知是否完全不顯示
- Windows 通知被使用者停用、勿擾或專注模式時，是否有 API 可以偵測。
- Credential Manager 的加密機制（是否為 DPAPI）。
- plugins-workspace `plugin/secure-storage` 分支的內容與發布時程。
- `global-shortcut` 外掛使用的 `global-hotkey 0.8` 在衝突時的錯誤型別：本次讀的是 master（0.10.0）的原始碼。
