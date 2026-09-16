# SNS 相册渲染测试

生产 album_render::render 接收联系人和帖子数组，返回 HTML 与相册帖子数。渲染层不访问文件、环境、网络或进程；媒体存在性、内容和链接检查由下载及发布层负责。

只接受 images 或 videos 下符合类型的直属文件名，过滤远端、绝对、跨层和非法路径。文件名按 UTF-8 URL 编码；safe_stem 保持文件名转换语义，但不是完整输出目录安全策略。

测试覆盖表情、未知表情、Python 标量转换、完整 HTML golden、空时间线、文本/图片/视频、年份顺序、注入、路径与尺寸异常。oracle 只用 AST 提取纯函数，写入由内存对象接管，不启动供应商模块或执行下载。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --bin wx application::moments::album_render::tests -- --nocapture
```

独立 Cargo.toml 也直接编译生产模块。`golden.json` 是迁移时固定的合成渲染契约，普通核对不重建预期。

## 视觉检查

preview.html 使用合成图片；placeholder.mp4 是空占位文件，只能检查播放器布局，不能证明视频可播放。桌面和手机视口分别检查文字、年份导航、媒体尺寸、溢出及可操作性。渲染成功不等于浏览器视觉或播放验收通过。
