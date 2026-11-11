# 六款造型素材 v2 交付

日期：2026/11/11。依據 docs/briefs/astra-skins-v2-brief.zh-TW.md，六款固定 ID 的素材全數交付。前端請以本資料夾為新版素材來源，舊 characters 與 cyber-penpal 概念圖保留作歷史參考。

## 檔案與使用方式

六個資料夾：leaf-cloak、moon-pebble、paper-lantern、night-letter、orbit-courier、prism-scribe。

每款均包含：

- idle.svg、listening.svg、processing.svg、approval.svg、suggestion.svg、paused.svg。
- 同名 -48.png、-96.png、-192.png，共 18 張透明 PNG。
- collapsed.svg：12 × 48；avatar.svg：96 × 96 座標系，供 24–40 px 顯示。
- skin.json：簡報要求的 ID、名稱、family、preferredTheme、accent、onAccent；夜航系列另有 glow。
- simplified/：六個 48 px 使用的簡化向量原稿。前端在 48 px 使用 -48.png，若要直接使用 SVG 則選此目錄。
- preview.png：512 px 透明向量渲染預覽，不是原先深藍底的生成概念圖。
- states-48-light.png、states-48-dark.png：六狀態以真實 48 px、DPR 1 並排的驗收截圖。
- verification.html：可開啟的個別造型檢視頁。

index.html 與 index.png 提供六款總覽；night-voyage-theme.html 與同名 PNG 提供夜航深淺介面示意。這些驗收頁與示意圖有背景，不能當作透明角色素材使用。

manifest.json 為素材索引補充，固定檔名仍依簡報，不要求前端改用新的契約。未修改應用程式 IPC、核心程式或其他 Agent 的工作檔案。

## 角色與狀態

48 px 版使用實心色塊、無模糊濾鏡、最小 4 個 SVG 單位的線寬（縮至 48 px 為 2 px）。夜航保留筆尖與信封輪廓、星軌信使保留琥珀環軌、稜鏡書靈保留翡翠書脊與書籤。96／192 px 版的夜航系列增加局部漸層，沒有整體透明度或外部背景。

所有款式採雙層對比輪廓，使淺色與深色桌布都有輪廓可見。原葉披風依統一畫布、安全區與狀態標記重新整理；保留葉尖、面部與披風設計，但不保證與舊 SVG 逐像素相同。

六種狀態的本體 SVG 內容完全一致，僅外部標記變化：

| 狀態 | 幾何辨識 |
| --- | --- |
| idle | 基本角色 |
| listening | 兩側接收弧線；指輸入框取得焦點，不代表開啟麥克風 |
| processing | 上方循環箭頭 |
| approval | 右下方驚嘆號方牌 |
| suggestion | 右上方四角星 |
| paused | 右下方雙直線暫停牌 |

paused 的 tooltip 依實際情況區分「勿擾」與「觀察已暫停」。標記之外仍需搭配可讀狀態文字，不能只靠角色推斷執行結果。素材為靜態，不含自動播放動畫。

## 配色與對比

night-voyage-tokens.json 與舊 design-tokens.json 具有相同頂層結構，完整保留 spacing、radius、size、type、motion；themes.light 與 themes.dark 各包含全部 13 個指定色彩欄位。

切換造型時使用 family 決定介面基底，再以該 skin.json 的 accent／onAccent 覆寫主要按鈕配色；不要將角色插畫填色直接作為文字色。preferredTheme 是偏好提示，不應覆蓋使用者已明確選定的主題。

contrast-report.json 記錄 44 組夜航介面文字配色與 12 組造型按鈕配色，共 56 組，全部至少 4.5:1，最低 5.760:1。文字檢查包含 text、muted、primary、secondary、success、warning、error 對 canvas、surface、subtle，以及 onPrimary 對 primary。border 與角色插畫不是文字色，不包含在此文字對比聲明內。

各款按鈕的精確比值另見 contrast-summary.md，數值由相對亮度公式計算，沒有四捨五入後才判斷通過。

## 應用程式圖示

固定採用夜航，放在 app-icon/app.svg、app-16.png、app-32.png、app-48.png、app-256.png，另附含四個尺寸的 app.ico。固定圖示不隨六款造型切換。系統匣沿用 docs/design/icons/tray-dark.svg 與 tray-light.svg。

此為設計交付決定，未改動 Tauri 設定；請外殼整合負責人將新版 app-icon 資產接入打包流程，避免誤用舊 icons/app.svg 的葉披風圖示。

## 驗證結果與界線

已執行：

```text
node docs/design/build-skins-v2.cjs
node docs/design/render-skins-v2.cjs
```

結果：六款完整產出；56 組對比通過；118 張角色、預覽與應用圖示 PNG 通過透明檢查。每張檢查四角 alpha=0、至少 25% 像素完全透明，且角色包含實心像素。詳細尺寸、透明像素數與邊界見 asset-validation.json。

36 個狀態 SVG 的本體以字串比對確認每款六狀態一致；匯出時拒絕 SVG 的 script、foreignObject、image 與外部 HTTP 參照。SVG 只有本機漸層，無字體、點陣圖、濾鏡或外部資源。所有 48 px 狀態在深淺背景總覽進行視覺檢查，並調整夜航的信封折線以避免遮住眼睛。

尚未驗證：前端實際造型切換、Tauri 透明視窗、桌面點擊穿透、多 DPI、系統匣與安裝包圖示顯示。素材與配色交付已完成，不代表上述產品整合完成。

重建工具依賴沿用 docs/design/.tools，未安裝到應用專案。渲染器目前使用 Windows Edge 的既有安裝路徑。修改 build-skins-v2.cjs 後重建會覆寫本資料夾的衍生素材；請在該來源修改，不只手改 PNG。
