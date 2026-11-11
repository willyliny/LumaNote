# 六款 2.5D 精靈待命素材交付

日期：2026/11/12。夜航方向獲使用者認可後，新增星軌信使、稜鏡書靈、葉披風、月環石、紙燈的2.5D分層版本。六款各自保留既有平面版，並列於 layered-collection.html。此為待命樣品及材質設計，不代表六個任務狀態已全部轉換成2.5D。

## 造型與體積

| 固定 ID | 名稱 | 材質方向 | @1x＋@2x 圖層合計 bytes |
| --- | --- | --- | --- |
| night-letter | 夜航 | 青藍玻璃摺信，淡紫反射 | 442349 |
| orbit-courier | 星軌信使 | 琥珀金球體、暖白眼睛、前後環軌 | 292786 |
| prism-scribe | 稜鏡書靈 | 翡翠玻璃封面、薄頁、發亮書脊 | 355483 |
| leaf-cloak | 葉披風 | 柔霧葉片、捲曲葉尖、奶油色面部 | 339941 |
| moon-pebble | 月環石 | 磨砂暖白石體、藍銀月環 | 262721 |
| paper-lantern | 紙燈 | 暖光透紙、摺角、紙面厚度 | 331556 |

每款均為8層，圖層預算獨立低於600000 bytes。新增五款總計80張透明圖層PNG，各款兩種解析度合計16張。預算不包含合成圖、驗收圖與製作來源。

## 每款交付內容

固定位置：`docs/design/skins/<ID>/layered/`。

- layers.json：沿用夜航簡報格式、384畫布、同一原點、pivot 0.5／0.62。
- 八層各一張192px @1x.png與384px @2x.png。
- composite.png：384px透明合成，排除閉眼圖層。
- on-light.png、on-dark.png、on-vivid.png：128 CSS px畫布在純白、純黑、鮮豔粉紅背景的驗收截圖。
- preview.html：可播放待命漂浮與眨眼、可暫停的獨立預覽頁。
- review-board.png：各款的放大材質與實際尺寸展示。
- validation.json、motion-validation.json：透明留白、尺寸、位元組數及動態驗證結果。
- source/：原創可編輯向量分層來源，僅供製作重建，前端使用烘焙PNG。

所有層同尺寸、同原點，無需額外對位。新增五款圖層順序為 shadow、glow、body-back、body、highlight、eyes-open、eyes-closed、accessory。body-back 的role為body；其餘role按名稱。夜航原有pen的role為accessory，前端必須依role分配動態，不能依檔名寫死pen。

睜眼預設opacity=1，閉眼預設opacity=0；兩者互斥顯示。眼睛分層不包含其他部位，切換時本體不變。

## 材質與動態範圍

五款皆由原創向量插畫以漸層、切面與局部高光建立2.5D層次，再離線轉成PNG；未從深色概念圖摳取角色，也沒有使用完整3D模型。自然系列保留各自材質，不套用相同玻璃表面。

球體與月環的厚度、書的書脊與紙頁、葉片的捲曲與重疊、紙燈的折面均已畫在本體裡。裝飾層分別是光帶、書籤、頂部葉片、小星點與紙張折角。只有輕微跟隨漂浮，不做星軌旋轉或書頁形變；這些進階動畫留待後續設計。

光暈與柔影烘焙在圖層內；前端僅transform與opacity。預覽上下約3px、4秒，傾斜正負1度，4–7秒隨機眨眼150ms。30秒無操作或visibilitychange隱藏時停播。產品需用Tauri實際視窗事件管理生命週期。

128px畫布內，新增五款的主要角色與裝飾約80–90px可視高度；夜航約95px。這是個別輪廓的視覺差異，不可逐圖裁切再疊放；若產品要統一視覺重量，請縮放整個圖層群組，保留共同原點與動態留白。四周12%的圖層保留區已驗證全透明。

48px以下、使用者開啟減少動態或素材載入失敗，產品繼續使用原平面版。驗收頁在減少動態時停止動畫但保留2.5D供對照，不代表改變產品的回退規格。

## 驗證結果

已對五款各自執行：

```text
node docs/design/build-other-layered.cjs
node docs/design/render-night-letter-layered.cjs <ID>
node docs/design/validate-night-letter-motion.cjs <ID>
node docs/design/review-layered-collection.cjs
```

通過：8層、兩種尺寸、12%透明邊界、檔案預算、圖層載入、無CSS即時模糊、只使用transform／opacity動畫、睜閉眼互斥、手動暫停及reduced-motion停播。visibilitychange處理函式使用合成事件測試，不等同原生桌面視窗隱藏測試。

已目視檢查所有角色在白、黑與鮮豔粉紅背景的合成結果，未見概念圖背景殘留。材質亮邊與設計光暈屬角色本身，不應作為去除對象。沒有宣稱任意桌布或所有DPI都已驗證。

尚未完成：產品內造型切換、原生視窗與輪盤尺寸、實機常駐GPU量測、其他五種任務狀態的2.5D素材。這次未修改產品前端、Rust外殼、IPC或平面版資產。

## 重建與接入

build-other-layered.cjs 是五款插畫與預覽頁的來源；修改後會覆寫各款layered/source、layers.json及preview.html。render-night-letter-layered.cjs與validate-night-letter-motion.cjs現在接受固定ID參數，省略參數仍處理原夜航，保持舊重建命令相容。

應用程式固定圖示仍使用夜航，不隨此次材質系列變更。造型偏好與模型、記憶、任務、授權保持獨立。前端只需依各款layers.json載入圖層，不要將驗收頁或有背景的review-board當作角色圖使用。
