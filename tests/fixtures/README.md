# 测试样本（fixtures）

这里放的是**真实抓取/真实合成的产物**，不是手写的假数据。为什么要留真数据：
mock 出来的响应永远「刚刚好」，而线上接口返回的是缺字段、多字段、字段里塞
HTML 的脏数据 —— 解析器的鲁棒性只有拿真实响应才能验证。

| 文件 | 大小 | 用途 | 怎么来的 |
| --- | --- | --- | --- |
| `youdao-real-response.json` | 60 KB | 有道词典的真实响应。解析三级阶梯（匹配式 / 启发式 / 纯文本）的离线回归基准 | 抓一次真实查询存下来 |
| `piper-zh-sample.wav` | 62 KB | 中文神经语音的合成产物基准，用来比对「换语音包 / 升级引擎后声音有没有坏」 | piper 用 `zh_CN-huayan-x_low` 合成 |
| `piper-en-sample.wav` | 45 KB | 英文合成产物基准，同上 | piper 用英文语音合成 |
| `piper-zh-phoneme-missing.txt` | 546 B | **一个未修 bug 的证据**，见下 | piper 的 stderr |

## `piper-zh-phoneme-missing.txt` 是什么

piper 合成中文数字时缺音素，报的是：

```
Missing 2 phoneme(s)
```

现象是 **2 和 5 读不出来**（其他数字正常）。这不是配置问题，是 `zh_CN-huayan-x_low`
这个语音包本身的音素表不全。

留着它是因为它是一个**可复现的失败样本**：以后换语音包、升级引擎、改前端的
数字预处理，都能拿它验证「这个 bug 还在不在」。删了就只能靠记忆，而记忆会漂。

## 语音模型放哪

19.7 MB 的 `zh_CN-huayan-x_low.onnx` **不在这里** —— 它在 `vendor/voices/`。
`/vendor/` 已写进 `.gitignore`，因为这类第三方二进制进仓库会让 clone 永久变重，
而它每次都能重新下载。这里的 wav 只有几十 KB，是「产物」不是「引擎」，所以入库。
