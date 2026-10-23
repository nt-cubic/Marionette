package redact

import (
	"strings"
	"testing"
)

// Chinese names are masked with the model on, both those it finds and
// those it misses. modelSaid is what the real model (privacy-filter q8,
// threshold 0.5, on an M4 Pro) found in each sentence, run on as it was.
func TestChineseNames(t *testing.T) {
	modelSaid := map[string]string{
		"王芳，她下周一": "private_person",
		"刘洋":      "private_person",
		"赵丽颖":     "private_person",
		"欧阳娜娜":    "private_person",
		"司马光":     "private_person",
		"张伟先生":    "private_person",
		"周杰伦":     "private_person",
		"田中太郎":    "private_person",
		// as it might: a name spaced out, a surname and a title
		"张 伟": "private_person",
		"李老师": "private_person",
	}
	f := &fakeModel{values: modelSaid}
	o := Options{Personal: true, Model: f.find}
	for _, c := range []struct {
		s     string
		names []string // masked
		keep  []string // left as they are
	}{
		{"帮我写封信给张伟，他住在北京市朝阳区建国路88号", []string{"张伟"}, []string{"，他住在"}},
		{"请把合同发给王芳，她下周一到上海。", []string{"王芳"}, []string{"，她下周一到上海"}},
		{"李娜和刘洋昨天开会讨论了项目进度。", []string{"李娜", "刘洋"}, []string{"和", "昨天开会"}},
		{"客户陈建国反馈导出功能在他电脑上失败了。", []string{"陈建国"}, []string{"反馈导出"}},
		{"客户张三反馈导出失败", []string{"张三"}, []string{"反馈"}},
		{"联系人：赵丽颖，手机", []string{"赵丽颖"}, nil},
		{"提交者是欧阳娜娜，审核人是司马光。", []string{"欧阳娜娜", "司马光"}, []string{"审核人是"}},
		{"张伟先生，您好！感谢您的来信。", []string{"张伟"}, []string{"先生，您好"}},
		{"我叫黄晓明，是这个仓库的维护者。", []string{"黄晓明"}, []string{"我叫"}},
		{"// author: 周杰伦", []string{"周杰伦"}, nil},
		{"给小王发个消息，说老李明天不来了。", []string{"小王", "老李"}, []string{"发个消息", "明天不来了"}},
		{"马云说过，今天很残酷。", []string{"马云"}, []string{"说过"}},
		{"田中太郎さんに連絡してください。", []string{"田中太郎"}, nil},
		{"转交张 伟处理", []string{"张 伟", "伟"}, nil},
		{"感谢李老师的帮助", []string{"李老师", "李"}, nil},
		// none of these is a name
		{"这个函数的作用是把高度转换成像素。", nil, []string{"高度"}},
		{"王国的军队在黄河边扎营。", nil, []string{"王国", "黄河"}},
		{"我们需要一个更高效的排序算法。", nil, []string{"高效"}},
		{"请帮我修复这个 bug，张开的括号没有闭合。", nil, []string{"张开"}},
		{"给张量赋值之前先检查形状。", nil, []string{"张量"}},
		{"在建国路上，路边有一家店。", nil, []string{"建国路", "路边"}},
		{"把方法和文件名都改一下", nil, []string{"方法", "文件"}},
		{"给老师发个消息", nil, []string{"老师"}},
		{"王老板说这个需求下周要上线，邓总明天要来看演示。", []string{"王老板", "邓总"}, []string{"明天要来"}},
		{"请帮我回复一下孙经理的邮件", []string{"孙经理"}, []string{"的邮件"}},
		{"抄送给吴敏和郑浩。", []string{"吴敏", "郑浩"}, nil},
		{"负责人：许文强", []string{"许文强"}, nil},
		{"我是郭靖，想咨询一下退款的问题。", []string{"郭靖"}, []string{"想咨询"}},
		{"和平时一样，周五发版。", nil, []string{"周五", "和平"}},
		{"方总结一下：说明文档里要写清楚安装步骤。", nil, []string{"方总结", "说明"}},
		{"运行测试的时候，内存一直在涨。签名和工具调用都会保留。", nil, []string{"签名和工具"}},
	} {
		out, _ := Mask(c.s, o)
		for _, n := range c.names {
			if strings.Contains(out, n) {
				t.Errorf("%q: %q not masked: %q", c.s, n, out)
			}
		}
		for _, k := range c.keep {
			if !strings.Contains(out, k) {
				t.Errorf("%q: %q masked: %q", c.s, k, out)
			}
		}
		if back := Restore(out, false); back != c.s {
			t.Errorf("round trip: %q", back)
		}
	}
}

// Without the model, or with personal data off, no Chinese name is
// looked for.
func TestChineseNamesNeedModel(t *testing.T) {
	const s = "客户钟离秋反馈导出失败" // a name no other test masks: one masked once stays masked (known.go)
	if out, _ := Mask(s, Options{Personal: true}); out != s {
		t.Fatalf("no model: %q", out)
	}
	f := &fakeModel{}
	if out, _ := Mask(s, Options{Secrets: true, Model: f.find}); out != s {
		t.Fatalf("personal data off: %q", out)
	}
}
