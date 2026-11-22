# Simplified Chinese (zh_CN) glossary for greycard's window

This is the glossary of a draft translation that no native speaker has
read yet. Corrections to it and to the catalog are welcome.

Source of the terms: the Simplified Chinese Lightroom Classic (LrC)
first, Capture One (C1) and darktable (dt) where LrC has none, and
the words mainland photographers use where the software has none.

## Conventions

- **Address.** Buttons, menu items, and commands: a short verb or
  verb-object phrase, as mainland desktop software writes them
  (导出..., 打开文件夹..., 全部复位). A question in a sheet's title:
  the plain question (“删除 3 张照片？”). Status lines: impersonal,
  a done action with 已 (“已导出 3 张照片”), one under way with 正在
  (“正在导出 3 张照片...”). “您” is never used; a sentence that must
  address the user is written impersonally (“有了预览，脱机的驱动器也能
  选片”, “已是最新版本”); “你” would be the choice if one ever must
  speak to the user, and no line needs it yet.
- **Punctuation.** Full-width in sentences: ，。：；？！ and “” for
  quotations; （） around text that holds Chinese, half-width () around
  text that is only Latin, digits, or keys ((F11), (?), (--no-sidecars)).
  Enumerations of nouns take 、 where the English lists with commas, and
  和 or 或 before the last. Menu ellipses stay as the source has them
  (“...” three dots, “…” where it has “…”), with no space before.
- **Spaces.** One space between Chinese and Latin letters, digits, or a
  placeholder that holds them ({n}, {name}, {file}, {label}, {size}):
  “3 张照片”, “导出到 {folder}”. No space next to full-width
  punctuation, none between a number and %, and none around a
  placeholder that is filled with Chinese words ({what}, {why}, {how},
  {place}, {words}, {done}).
- **Tails.** An English tail joined on with “, ” or “; ” (`, {} backed
  up`) starts with ， or ； and no space, so the joined line reads as
  one Chinese sentence. A tail joined with a leading space alone (`
  Refitting {name}.`) drops the space when it follows a full stop.
  A list's middle item takes 、 (`, {n} camera JPEG` → `、{n} 张相机
  JPEG `, its trailing space keeping “JPEG 和” apart). Where the Rust
  joins with a literal English “, ” or “; ” outside the catalog (lists
  of file names, a look's reasons), the joined line keeps that
  half-width mark.
- **Headings.** The develop panel's ALL CAPS headings are plain Chinese
  (WHITE BALANCE → 白平衡); Chinese has no capitals.
- **Product words.** greycard, Lightroom, RawTherapee, darktable,
  lensfun, GitHub, ONNX Runtime, JPEG, TIFF, PNG, DNG, ICC, EXIF, XMP,
  RGB, HDR, GPU, CPU, file names, units (MB, KB, GiB, px, EV, K, mired,
  s), and the search's tokens (`camera:`, `iso>=`) stay as they are.
- **Keys.** Key names stay as the keys sheet draws them (Ctrl, Shift,
  Alt, Space, Enter, Esc, Tab), as docs/translating.md asks. The
  pointer's word is translated: “Ctrl+单击”, “右键单击”.
- **Numbers and dates** keep the English form (§616).
- **Plurals.** nplurals=1; plural=0: one form, the count before a
  measure word.
- **Measure words.** 张 for frames, pictures, and thumbnails (3 张照片);
  个 for files, sidecars, folders, presets, looks, models, masks, and
  shapes; 组 for an export queue's sets; 项 for edits and abstract
  items (3 项编辑); 次 for repeated actions (3 次撤销).
- **Frame / picture.** A frame is 照片 (LrC); the picture, what is
  drawn, is 图像 (加在图像之外). A count of pictures is 张照片 as well.

## Develop

**修改照片**, as LrC's module is named. The shorter 冲印 reads as
printing to most mainland users, and 修图 as retouching (a separate tab
here, 修饰); 修改照片 is four characters, about the width of the
English “Develop”, so the Loupe's mode row keeps its width. The verb
“develop” (a raw being rendered for the screen or an export) is 处理:
“正在处理...”, “已处理”. The hint “Enter develops” is “Enter 修改照片”.

## Terms

| English | Chinese | Why / alternative |
| --- | --- | --- |
| Library | 图库 | LrC |
| Develop (the mode) | 修改照片 | LrC; see above |
| develop (render) | 处理 | alt. 渲染 |
| Loupe | 放大视图 | LrC “放大视图” |
| Cull / Culling | 选片 | photographers' word; LrC has none (its 筛选视图 is Survey) |
| Lightbox | 灯箱 | standard |
| Filmstrip | 胶片栏 | LrC “胶片显示窗格”, too long for menus |
| Grid | 网格 | LrC “网格视图” |
| Panel / pane | 面板 / 窗格 | LrC |
| Exposure | 曝光度 | LrC |
| Contrast | 对比度 | LrC |
| Highlights | 高光 | LrC |
| Shadows | 阴影 | LrC |
| Midtones | 中间调 | LrC Color Grading |
| Whites / Blacks | 白色色阶 / 黑色色阶 | LrC |
| Clarity | 清晰度 | LrC |
| Texture | 纹理 | LrC |
| Dehaze | 去朦胧 | LrC |
| Vibrance | 自然饱和度 | LrC |
| Saturation | 饱和度 | LrC |
| White balance | 白平衡 | LrC |
| Temperature / Tint (WB) | 色温 / 色调 | LrC |
| As shot | 原照设置 | LrC |
| Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash | 日光、阴天、阴影、钨丝灯、荧光灯、闪光灯 | LrC; Shade's 阴影 is the same word as Shadows, as in LrC |
| Neutral (WB dropper) | 中性色 | |
| TINT (the section that lays a color over) | 着色 | not 色调, which is WB tint; alt. 色彩叠加 |
| Light (the section) | 光线 | LrC's Basic panel says 色调; 光线 as its newer “Light” panel |
| Highlight fill | 高光填充 | none in LrC; alt. 高光重建 (dt) |
| Display curve | 显示曲线 | none in LrC |
| Per channel | 按通道 | |
| Curves / Tone curve | 曲线 / 色调曲线 | LrC |
| Point / Parametric | 点 / 参数 | LrC “点曲线 / 参数曲线” |
| Lights / Darks (parametric) | 亮色调 / 暗色调 | LrC |
| Color mixer | 混色器 | LrC |
| Hue / Luminance | 色相 / 明亮度 | LrC HSL |
| Luma / Chroma | 亮度 / 色度 | dt; LrC's noise panel says 明亮度 / 颜色, which collides with Luminance |
| Color grading | 颜色分级 | LrC |
| Balance (grading) | 平衡 | LrC |
| Black & white | 黑白 | LrC |
| Filter (B&W's color filter) | 滤镜 | as a lens filter |
| Sharpen | 锐化 | LrC |
| Output sharpen | 输出锐化 | LrC export “输出锐化” |
| Amount / Radius / Detail / Threshold | 数量 / 半径 / 细节 / 阈值 | LrC |
| Noise | 杂色 | LrC “减少杂色” |
| Denoise | 降噪 | LrC “降噪” |
| learned (the AI models' work) | AI | “AI 降噪” as LrC's |
| Structure (noise and detail slider) | 结构 | |
| Detail (the section) | 细节 | LrC |
| Grain | 颗粒 | LrC |
| Vignette (the effect) | 暗角 | LrC “裁剪后暗角” |
| Vignetting (the lens's) | 晕影 | LrC lens corrections |
| Midpoint / Feather / Roundness | 中点 / 羽化 / 圆度 | LrC |
| Demosaic | 去马赛克 | dt |
| Lens corrections | 镜头校正 | LrC |
| Distortion | 扭曲度 | LrC |
| Chromatic aberration | 色差 | LrC |
| Defringe | 去边 | LrC |
| Crop | 裁剪 | LrC |
| Aspect | 长宽比 | LrC |
| Guides | 参考线 | LrC “裁剪参考线叠加” |
| Rotate (part of the edit) | 旋转 | LrC |
| Turn (a quarter, not part of the edit) | 转向 | kept apart from 旋转 |
| Flip | 翻转 | LrC |
| Level | 拉直 | LrC |
| Perspective | 透视 | LrC Transform |
| Mask / Masks | 蒙版 | LrC |
| Shape (of a mask) | 形状 | |
| Adjustment (a mask's row) | 调整 | |
| Global | 全局 | |
| Linear / Radial | 线性渐变 / 径向渐变 | LrC |
| Brush / Paint | 画笔 / 绘制 | LrC |
| Erase / Add / Subtract / Intersect / Invert | 擦除 / 添加 / 减去 / 相交 / 反相 | LrC |
| Subject / Sky / Background / People / Object / Landscape | 主体 / 天空 / 背景 / 人物 / 对象 / 风景 | LrC |
| Color range / Luminance range | 颜色范围 / 明亮度范围 | LrC |
| Refine edge | 调整边缘 | Photoshop |
| Retouch (the tab) | 修饰 | alt. 修复 (collides with Heal) |
| Heal / Clone / Fill | 修复 / 仿制 / 填充 | LrC removal; Photoshop 仿制图章 |
| Remove (the retouch tool's work) | 移除 | LrC “移除” |
| Patch | 修补 | Photoshop |
| Look | 风格 | C1 Styles; LrC's 配置文件 means camera profile here |
| picture style (a camera's) | 照片风格 | Canon's own Chinese |
| Camera profile | 相机配置文件 | LrC |
| Preset | 预设 | LrC |
| Reset | 复位 | LrC |
| Snapshot | 快照 | LrC |
| History | 历史记录 | LrC |
| Version (a virtual copy) | 版本 | LrC “虚拟副本” for the act |
| Sidecar | 附属文件 | Adobe's XMP word |
| Edit (noun) | 编辑 | alt. 编辑设置 |
| raw (the file) | RAW / RAW 文件 | as Chinese photographers write it |
| Remove / Remove from | 移除 / 移去 | 移除 for what goes away (a snapshot, a look, a shape); 移去 for taking out of a collection, stack, group, or the library |
| Copy / Duplicate | 拷贝 / 复制 | LrC: 拷贝设置, 复制 for a duplicate |
| Favorites (presets) | 常用 | 收藏 is the collections' word |
| Section / part (sync, presets) | 部分 / 项 | |
| Fit (camera match) | 拟合 | |
| Highlight fill / fill model | 高光填充 / 填充模型 | |
| Capture (a screenshot run) | 截图运行 | |
| Rating / stars | 星级 / 星 | LrC |
| Flag / Pick / Reject / Unflag / Unflagged | 旗标 / 留用 / 排除 / 取消旗标 / 无旗标 | LrC |
| Rejects (the frames) | 排除的照片 | 排除照片 where a second 的 would follow (排除照片的副本) |
| Rejects folder | 排除文件夹 | |
| Label | 色标 | LrC |
| Keyword | 关键字 | LrC |
| Title | 标题 | LrC |
| Stack | 堆叠 | LrC |
| Collection / Smart collection / Collection set | 收藏夹 / 智能收藏夹 / 收藏夹集 | LrC |
| Root (a library root) | 根文件夹 | |
| Archive / Back up / Bring back | 归档 / 备份 / 取回 | |
| Card | 存储卡 | |
| Trash | 回收站 | Windows' word; macOS says 废纸篓 |
| Filter (the filter bar) | 过滤 / 过滤器 / 过滤条件 | LrC “图库过滤器”; 过滤条件 for a filter saved as a smart collection; 筛选 is kept off it so Cull's 选片 stays apart |
| Style (a facet chip and the vignette's style) | 样式 | one entry serves both; the camera's picture style elsewhere is 照片风格 |
| Sync | 同步 | LrC |
| Copy settings / Paste settings | 拷贝设置 / 粘贴设置 | LrC |
| Export / Import | 导出 / 导入 | LrC |
| Export queue / set | 导出队列 / 组 | |
| Soft proof | 软打样 | LrC |
| Gamut warning | 色域警告 | LrC |
| Intent / Perceptual / Relative | 方法 / 可感知 / 相对 | LrC soft proofing |
| Long edge | 长边 | LrC |
| Watermark | 水印 | LrC |
| Metadata | 元数据 | LrC |
| Capture time | 拍摄时间 | LrC |
| Shift capture time | 调整拍摄时间 | LrC “编辑拍摄时间” |
| Navigator | 导航器 | LrC |
| Merge to HDR / Panorama / Focus stack | 合并到 HDR / 合并到全景图 / 合并到焦点堆叠 | LrC “照片合并” |
| Canvas | 画布 | LrC says 背景, which is the mask's Background here |
| Scope / Selection (scopes) | 示波器 / 选区 | |
| Clipping (warnings) | 修剪 | LrC “高光修剪 / 阴影修剪” |
| Focus peaking | 峰值对焦 | camera menus |
| Thumbnail / Preview | 缩略图 / 预览 | LrC |
| Library index | 图库索引 | |
| offline / isn't reachable | 脱机 / 无法访问 | Adobe |
| Model (AI) | 模型 | |
| Settings | 设置 | |
| Keys | 快捷键 | |
| Log | 日志 | |
