# 云母品牌资产

2026-09-12：按用户选择的「更浓的动漫感，角色更突出」方向，用内置 `image_gen` 生成并接入以下素材。

| 素材 | 角色与用途 |
| --- | --- |
| `art/kirara-icon.png` | 云母 Q 版头像；应用品牌入口、网页 favicon。以组合插画中的云母为参考生成。 |
| `art/companions.webp` | 犬夜叉、戈薇、云母；追剧订阅空状态及宽屏页头，保留生成图的透明通道。 |
| `art/journey.webp` | 杀生丸、玲、邪见；侧栏樱花山路插画，较矮窗口隐藏以优先展示菜单。 |

精确提示词见 `art/PROMPTS.json`、`art/kirara-icon.prompt.json`，PNG 内嵌提示词，WebP 的提示词同时记录在相邻 `.json` 文件中。图像只进行了尺寸压缩和格式转换；网页素材均在本仓库，不依赖生成器目录或外部图片地址。

桌面图标 `src-tauri/icons/` 中的 PNG、ICO 和 ICNS 由同一张新云母图通过 Tauri icon 命令生成；提示词内嵌或存放在相邻 sidecar 中。

品牌入口通过「云母」文字提供可访问名称，角色插图与界面品牌图标通过 CSS `background-image` 呈现，容器使用 `aria-hidden="true"`；影视海报继续使用语义化 `<img>`。菜单功能图标沿用 Lucide 线性图标，避免角色图片影响操作辨识。角色本色独立于 UI 语义色。

旧版代码绘制的 `yunmu-icon.svg` 与 `kirara-rest.svg` 保留为历史素材，当前应用不再引用。
