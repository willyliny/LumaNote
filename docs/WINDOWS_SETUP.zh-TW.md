# 精靈助手：Windows 11 第一次建置與執行

這份文件給「第一次在自己的 Windows 11 電腦上把 Tauri 外殼跑起來」的人，不需要懂 Rust 或 Tauri。
目標只有一個：確認工具鏈能用，並看到三個視窗與系統匣的基本行為。**這一版還沒有接上任何真正的 AI 功能**，畫面裡的內容是示範用的假資料。

> 重要：這個外殼是在 Linux 上寫的，第一版已在你的 Windows 11 上成功編譯並執行；但每一版新增的視窗行為（透明、放射選單、位置記憶）都還沒在 Windows 上驗證過。出現問題請依第 6 節把訊息貼回來，我們會修。

---

## 1. 需要先安裝的東西（只做一次）

| 項目 | 怎麼裝 | 怎麼確認 |
| --- | --- | --- |
| Microsoft C++ Build Tools | 下載「Visual Studio Build Tools」，安裝時勾選 **「Desktop development with C++」（使用 C++ 的桌面開發）** 工作負載 | 開始功能表搜尋「Developer PowerShell for VS」找得到即可 |
| Rust | 到 https://rustup.rs 下載 `rustup-init.exe` 安裝，預設值即可（必須是 **MSVC** 版）。裝完**關掉並重開 PowerShell** | `rustc --version` 要顯示 **1.90 或更新** |
| Node.js | 安裝 **LTS** 版 | `node -v` 要是 **v20.19 以上** 或 **v22.12 以上** |
| WebView2 | Windows 11 已內建，不用裝 | 無 |

在 PowerShell 檢查：

```powershell
node -v
npm -v
rustc --version
cargo --version
rustup show
```

`rustup show` 輸出中的 active toolchain 應該包含 `stable-x86_64-pc-windows-msvc`。如果不是，或版本低於 1.90，執行：

```powershell
rustup default stable-msvc
rustup update stable
```

（選用）想用指令安裝 Build Tools，可用 winget；用圖形介面安裝也可以，結果一樣：

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

---

## 2. 第一次執行

在 PowerShell 輸入：

```powershell
cd C:\Users\a0925\Desktop\phison_elf
npm install
npm run tauri dev
```

- `npm install`：下載前端套件，約 1 到 3 分鐘。
- `npm run tauri dev`：同時啟動前端開發伺服器並編譯 Rust 外殼。

**第一次編譯需要多久（估計值，尚未在你的電腦實測）**：約 5 到 15 分鐘，視 CPU 與網路而定，會下載並編譯幾百個 Rust 套件（包含一份內建的 SQLite，需要 C++ Build Tools）。中間看起來停住沒關係，只要終端機沒有出現 `error`，請耐心等。之後再次啟動通常只要幾秒到一分鐘。整個編譯過程會佔用數 GB 磁碟空間（`target` 資料夾）。

想把輸出同時存成檔案方便貼回來，可改用：

```powershell
npm run tauri dev 2>&1 | Tee-Object -FilePath tauri-dev.log
```

---

## 3. 成功時你應該看到什麼

編譯完成後會自動開啟：

1. **浮動小精靈**：一個很小的透明視窗（128 x 128，小精靈本體約 96 x 96，周圍留白供傾斜與漂浮），沒有邊框，永遠在最上層，不出現在工作列。可以用滑鼠按住拖曳。
2. **主面板**：約 420 x 720 的一般視窗，標題「精靈助手」，裡面是示範用的對話與今日畫面。
3. **系統匣圖示**：一滴青綠色水滴，滑鼠停上去會顯示「精靈助手（左鍵開啟，右鍵選單）」。**Windows 11 可能把它收在 `^`（顯示隱藏的圖示）裡面**，不一定直接顯示在工作列右下角（社群回報新程式的圖示常被放進這個溢出區；我沒有查到微軟官方文件明確寫出預設值，所以請以你的畫面為準）。找不到時，先按 `^` 展開。想讓它固定顯示：設定 → 個人化 → 工作列 → 展開「其他系統匣圖示」→ 把「精靈助手」（開發模式下可能顯示為 `elf-shell`）打開。也可以把圖示從 `^` 裡直接拖到工作列右下角。
4. **系統匣選單**：在圖示上按**右鍵**，應該出現 開啟、暫停觀察、勿擾、結束。「暫停觀察」與「勿擾」是可打勾的開關（目前只會記住打勾狀態，沒有實際作用）。

請逐項確認：

- 在系統匣圖示上按**左鍵** → 主面板顯示並移到最前面（右鍵才是選單）。
- 按主面板右上角的 **X** → 主面板**隱藏**，但程式**沒有結束**：小精靈與系統匣圖示還在，終端機也還在跑。這是設計：**只有系統匣的「結束」會結束程式**。關閉規則：
  - `main`（主面板）：按 X 只是隱藏。
  - `settings`（設定視窗）：按 X 只銷毀設定視窗本身，不影響其他視窗；主面板已隱藏時關掉設定視窗，程式仍然在跑。
  - `floating`（小精靈）：使用者關不掉它；即使它是唯一剩下的視窗，程式也不會結束。
  - 萬一所有視窗都消失，程式碼會攔下「最後一個視窗關閉」所造成的結束要求（只有 `app.exit` 才能結束）。
- 點小精靈 → 主面板重新出現。
- 在主面板按「設定」→ 另開一個設定視窗；關掉設定視窗後，再開一次仍然正常（每次都是重新建立）。
- **第二次啟動會把主面板叫出來，不會多一個程式**：保持第一個還在執行，**另開一個新的 PowerShell**，輸入下面指令（不要再執行 `npm run tauri dev`，那會因為 5173 連接埠已被占用而失敗，這是正常的）：

  ```powershell
  cd C:\Users\a0925\Desktop\phison_elf
  .\target\debug\elf-shell.exe
  ```

  預期：沒有出現新視窗或新圖示，而是原本的主面板被帶到最前面（如果它被隱藏了就會顯示出來）。
- 系統匣選單點 **結束** → 所有視窗與系統匣圖示消失，終端機的 `npm run tauri dev` 也結束（這才是真正離開）。

按 F12（或在主面板按右鍵 → 檢查）開啟開發者工具，Console 應該看到一行 `[elf] app_info {app_name: ..., app_version: "0.1.0", tauri_version: "2.12.5"}`。這代表前端真的呼叫到了 Rust。

### 這一版已知的樣子

- 小精靈視窗平常是 128 x 128；滑鼠停在上面時會變成 344 x 344（放射選單，六個項目與 96 px 小精靈之間至少 8 px），離開後縮回 128 x 128。小精靈中心位置不變；舊版（96 x 96 視窗）儲存的位置會自動換算，保持小精靈中心不動。
- 2.5D 造型（設定 → 助手造型，標示「2.5D」）會輕微漂浮、眨眼、滑鼠靠近時微傾；視窗隱藏或 30 秒沒有滑鼠動作時動畫暫停；系統開啟「減少動態」時改用平面圖。
- 小精靈的位置會記住（存在應用程式設定資料夾，檔名 `floating-position.json`，通常在 `%APPDATA%\com.phisonelf.assistant\`）。第一次執行、或存的位置已不在任何螢幕上（例如拔掉外接螢幕）時，會放在主螢幕工作區的右下角。
- 小精靈點下去只會開主面板，這是刻意的：它不能接收鍵盤輸入。

### 本版請檢查（新增）

1. **小精靈是透明的**：看得到桌面或後面的視窗，沒有方形底色，也沒有白框或黑框。
2. **滑鼠停在小精靈上**：放射選單的範圍出現，**小精靈本身在螢幕上不會跳動**。把小精靈拖到螢幕最右邊與最下面（貼著工作列）再試一次，視窗會被推回螢幕內，但小精靈仍應停在原地。
3. **拖曳仍然正常**：按住小精靈可以移動；選單展開時拖曳也不應壞掉。
4. **位置會記住**：把小精靈拖到別處，等一秒，完全結束（系統匣 → 結束）後重新啟動，它應該回到同一個地方。如果有外接螢幕：把小精靈放到外接螢幕、結束、拔掉外接螢幕、再啟動，它應該出現在主螢幕右下角。
5. **關閉只是隱藏**：主面板按 X 後小精靈與系統匣都還在；點小精靈或系統匣左鍵可叫回。開設定視窗再關掉，其他視窗不受影響。
6. **系統匣**：圖示在 `^` 裡或工作列上，有提示文字，右鍵選單四項齊全，「結束」才會真的離開。
7. 小精靈出現的瞬間不應該閃白色。

---

## 4. 要回報的實機觀察（我們還沒驗證過）

請順手看一下這幾件事，有異常就告訴我們：

1. 小精靈有沒有出現在**工作列**或 **Alt+Tab** 清單。
2. 拖曳小精靈時，目前正在打字的視窗會不會**瞬間失去焦點**。
3. 小精靈出現時有沒有**白色閃一下**，或背景不是透明的。
4. 小精靈**周圍透明的區域**會不會擋住下面視窗的點擊。
5. 系統匣圖示的顏色與清晰度是否正常。

---

## 5. 結束與重來

- 正常結束：系統匣選單 → 結束。
- 如果卡住：在終端機按 `Ctrl + C`。
- 如果下次啟動說 5173 連接埠被占用：代表上次的前端伺服器還活著。重新開機，或在工作管理員結束殘留的 `node.exe`。
- 想完全重編譯：`cargo clean`（會刪掉 `target`，之後又要等第一次編譯那麼久）。

---

## 6. 常見錯誤與要貼回來的資料

| 症狀 | 可能原因 | 怎麼處理 |
| --- | --- | --- |
| `linker 'link.exe' not found`、`could not find MSVC`、`cannot open input file 'kernel32.lib'` | 沒裝 C++ Build Tools，或沒勾到「Desktop development with C++」 | 重新執行 Visual Studio Installer，修改 Build Tools，勾選該工作負載（含 Windows 11 SDK），裝完重開 PowerShell |
| `rustc 1.xx is not supported`、`requires rustc 1.90` | Rust 太舊 | `rustup update stable` |
| `npm : 因為這個系統上已停用指令碼執行，所以無法載入檔案 npm.ps1` | PowerShell 執行原則 | 改用 `npm.cmd install`、`npm.cmd run tauri dev`，或執行 `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` |
| `Port 5173 is already in use` | 前端伺服器重複啟動（或上次沒關乾淨） | 關掉其他 `npm run dev` / `tauri dev`，或重開機 |
| 視窗打開但是**全白或顯示連線被拒絕** | 前端伺服器沒起來或位址不通 | 另開瀏覽器試 `http://127.0.0.1:5173/panel.html`，能開就代表伺服器正常，把終端機訊息與 F12 Console 內容貼回來 |
| `failed to run custom build command for elf-shell`，內文提到 permission、capability 或 icon | 外殼設定檔有誤（我們的問題） | 把這段錯誤整段貼回來 |
| `error: failed to download` / `timed out` / `SSL` | 網路、公司或學校代理、防毒擋掉 crates.io | 換網路再試一次；仍失敗就貼出完整訊息 |
| `EBADENGINE` 或 Vite 要求的 Node 版本警告 | Node 太舊 | 安裝 Node LTS（v20.19 以上或 v22.12 以上）。Vite 7 以上不支援更舊的 Node |
| `Cannot find native binding`（常見於 rollup、esbuild、lightningcss 等的原生套件） | `package-lock.json` 或 `node_modules` 是在別的作業系統（Linux）產生的，沒有包含 Windows 版的原生套件 | **鎖定檔必須在 Windows 上產生。** 在專案資料夾刪除 `node_modules` 資料夾與 `package-lock.json`，再執行 `npm install`（PowerShell：`Remove-Item -Recurse -Force node_modules; Remove-Item package-lock.json; npm install`）。完成後把新的 `package-lock.json` 一併交回，之後就不要再從 Linux 覆蓋它 |
| 小精靈看不到、圖示也找不到 | 圖示被收進 `^`；或小精靈的存檔位置異常 | 先按 `^` 找圖示；左鍵點它叫出主面板。要重設小精靈位置，結束程式後刪除 `%APPDATA%\com.phisonelf.assistant\floating-position.json` 再啟動 |
| 編譯過程中出現檔案被占用、存取被拒 | 防毒軟體掃描 `target` 資料夾 | 暫時把專案資料夾加入防毒排除，或重試一次 |
| `elf-core` 裡面的編譯錯誤 | 核心程式正在由其他人修改 | 這不是你的環境問題，貼回來即可 |

### 出問題時請貼回來這些

1. **完整**的終端機輸出：從第一行出現 `error` 的地方到最後（不要只貼最後一行）。如果用了第 2 節的 `Tee-Object`，直接給 `tauri-dev.log`。
2. 這五個指令的輸出：

   ```powershell
   node -v
   npm -v
   rustc --version
   cargo --version
   rustup show
   ```
3. 如果程式有跑起來但行為不對：描述哪一項不符合第 3 節，加上主面板 F12 Console 的內容，必要時附螢幕截圖。
