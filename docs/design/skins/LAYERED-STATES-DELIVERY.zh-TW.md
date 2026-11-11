# 六款精靈完整 2.5D 狀態素材

日期：2026/11/12。六款均完成待命、聆聽輸入、處理中、等待確認、有建議、勿擾／暫停六種2.5D呈現。原本的材質與本體圖層不變，新增30個獨立狀態標記，各含192／384px透明PNG，共60張新圖層。

## 前端資產入口

每款維持 `docs/design/skins/<ID>/layered/`，新增：

- states.json：狀態組合契約，schemaVersion=1。
- state-listening、state-processing、state-approval、state-suggestion、state-paused 各有 @1x.png、@2x.png。
- states/idle、listening、processing、approval、suggestion、paused 各有 @1x.png、@2x.png 靜態合成圖，供低成本預覽／驗收；產品的小尺寸與減少動態仍沿用平面版。
- states-preview.html、states-on-light.png、states-on-dark.png、states-on-vivid.png：128px畫布六狀態並排驗收。
- states-validation.json：本體未改動、透明邊界與預算結果。

總覽：layered-states.html／layered-states.png。狀態解析模組：layered-state-resolver.mjs，無依賴、無網路請求，可匯入前端或複製為型別化實作。

## 接入方式

目前應用的 Sprite.tsx 與 scripts/sync-skins.mjs 僅支援平面SVG；本次交付不會自動讓產品顯示2.5D。前端需把選定造型的layered資料夾隨應用資產打包，並新增分層渲染分支。不要把開發路徑當成正式安裝路徑。

```js
import { resolveLayeredState } from './layered-state-resolver.mjs';
// base = 該造型 layers.json；manifest = 同目錄 states.json
// 使用現有 APP_TO_SKIN_STATE，working→processing、awaiting→approval、dnd→paused。
const frame = resolveLayeredState(base, manifest, skinState, '2x');
// frame.layers 由底至頂，每個src相對於該造型layered目錄。
// 各層absolute inset:0，使用同一尺寸與frame.pivot；opacity依回傳值設定。
// layer.motion==='none' 不套動畫；role==='state' 必須保持靜止。
// frame.blink=false 時清除眨眼timer，使用回傳的眼睛opacity。
```

先預載目前造型的基底與五種標記，解析整個frame後一次更新。固定容器尺寸，不按圖片有效像素裁切，不逐層改變定位。狀態改變時先取消舊眨眼計時器再套新狀態，避免paused後眼睛又被打開。

在待命時保留原8層；其他狀態移除glow，加入1個state標記，仍為8個掛載圖層（包含opacity=0的另一組眼睛）。身體內的高光與反射保留。**不要把五個標記與glow全部加入layers.json後永久掛載。** layers.json保持原格式，由states.json描述狀態差異。

未知狀態回退idle；錯誤schema或造型ID不一致則丟出錯誤，呼叫端回退同款平面版。任何素材載入失敗也回退平面版，不能留下半個角色。

## 狀態語意

| 狀態 | 標記 | 眼睛與動態 |
| --- | --- | --- |
| idle | 無 | 睜眼、可漂浮與眨眼 |
| listening | 三條輸入接收線 | 睜眼；表示輸入框聚焦，不代表麥克風已開啟 |
| processing | 圓形循環箭頭 | 睜眼；本體可沿用待命漂浮，徽記靜止 |
| approval | 三角驚嘆號 | 睜眼；搭配明確的確認卡與可讀文字 |
| suggestion | 四角星 | 睜眼；不閃爍、不搶焦點 |
| paused | 雙直線 | 閉眼、禁止漂浮與眨眼 |

每款標記保留對應的青藍、琥珀、翡翠、葉綠、藍銀或暖金色，幾何圖案區分狀態，不只靠顏色。標記採同一右下方位置，96px級別可辨識；仍應搭配tooltip／aria-label。勿擾與暫停觀察由產品文字區分，不混同業務狀態。

## 預算與品質

數值包含每款原8層及新增5個標記的兩種解析度；靜態合成圖、預覽截圖與來源不計入圖層下載預算。前端按需載入，無需預載靜態合成圖。

| ID | 全部圖層 bytes | 上限 |
| --- | --- | --- |
| night-letter | 519926 | 600000 |
| orbit-courier | 373242 | 600000 |
| prism-scribe | 435132 | 600000 |
| leaf-cloak | 418819 | 600000 |
| moon-pebble | 338784 | 600000 |
| paper-lantern | 412229 | 600000 |

新增標記以同一384畫布製作，四周12%留白檢查通過，沒有背景方塊。標記的陰影／明暗已在PNG中，前端不需CSS filter或即時模糊。原本16張本體圖層逐檔SHA-256確認未改動，狀態切換沒有重畫本體。

驗證命令：

```text
node docs/design/build-layered-states.cjs
node docs/design/validate-layered-states.mjs
```

驗證包含72種造型／狀態／解析度組合、216種狀態轉換、每次8層、眼睛互斥、paused禁止動態、資產路徑存在、跨狀態本體來源一致、未知狀態回退與不相容設定拒絕。透明邊界、檔案預算與原圖雜湊另記於layered-states-validation.json。

已產生三種背景的全部36狀態驗收圖。實際Tauri視窗、DPI、GPU、同步腳本與產品React元件的整合仍需前端負責人完成；本次沒有修改其他Agent正在處理的產品程式。
