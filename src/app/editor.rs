//! 输入编辑器缓冲：多行文本 + 字符级光标 + kill-ring + undo（纯逻辑可测）。
//!
//! 快照策略：仅内容类操作（插入/换行/删除/kill/yank）前压 undo 栈——
//! 光标移动不入栈（规格只要求"编辑"可撤销），光标位置随内容快照一并恢复。

use std::collections::VecDeque;

/// undo 快照上限（超出丢弃最旧）。
const UNDO_CAP: usize = 100;
/// kill-ring 容量（新 kill 在前）。
const KILL_RING_CAP: usize = 32;

/// 内容快照：(行数组, 光标行, 光标列)。
type Snapshot = (Vec<String>, usize, usize);

/// 多行编辑器缓冲。
#[derive(Debug, Clone)]
pub struct EditorBuffer {
    lines: Vec<String>,
    /// 光标所在行（字符序号 col）。
    row: usize,
    col: usize,
    undo: Vec<Snapshot>,
    /// kill-ring：index 0 为最新。
    kill_ring: VecDeque<String>,
    /// yank 后 yank-pop 的游标（0=最新 kill）。
    yank_pos: usize,
    /// 上一次操作是否为 yank（yank-pop 仅紧随 yank 生效）。
    after_yank: bool,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorBuffer {
    /// 创建空缓冲（单个空行，光标原点）。
    ///
    /// 输入: 无
    /// 输出: 可直接使用的 EditorBuffer
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            undo: Vec::new(),
            kill_ring: VecDeque::new(),
            yank_pos: 0,
            after_yank: false,
        }
    }

    // ---------- 只读访问（view/提交用） ----------

    /// 全文（行以 \n 连接）。
    ///
    /// 输入: 无
    /// 输出: 缓冲完整文本
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// 缓冲是否完全为空（单个空行）——ctrl+d 退出判定，对齐 pi 的 length===0。
    ///
    /// 输入: 无
    /// 输出: true 表示无任何字符
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// 行数组（view 逐行渲染）。
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// 光标行号。
    pub fn row(&self) -> usize {
        self.row
    }

    /// 光标列（字符序号）。
    pub fn col(&self) -> usize {
        self.col
    }

    // ---------- 程序化修改（不入 undo） ----------

    /// 整体替换内容，光标移到末尾（set_editor_text 等程序化路径）。
    ///
    /// 输入: text — 新全文（可含 \n）
    /// 输出: 无返回值；副作用为替换行数组与光标
    pub fn replace(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(String::from).collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
        self.after_yank = false;
    }

    /// 在末尾追加一块文本（换行分隔），光标移到追加处——队列恢复用。
    ///
    /// 输入: text — 追加文本（可含 \n）
    /// 输出: 无返回值；副作用为追加行并移动光标
    pub fn append_block(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.is_empty() {
            self.replace(text);
            return;
        }
        for line in text.split('\n') {
            self.lines.push(line.to_string());
        }
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].chars().count();
        self.after_yank = false;
    }

    // ---------- 内容编辑（入 undo） ----------

    /// 在光标处插入一个字符。
    ///
    /// 输入: c — 待插入字符
    /// 输出: 无返回值；副作用为快照、插字、光标右移
    pub fn insert(&mut self, c: char) {
        self.push_undo();
        self.insert_char(c);
        self.after_yank = false;
    }

    /// 在光标处换行（拆分当前行）。
    ///
    /// 输入: 无
    /// 输出: 无返回值；副作用为快照、拆行、光标移到新行行首
    pub fn newline(&mut self) {
        self.push_undo();
        let byte = char_byte_idx(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
        self.after_yank = false;
    }

    /// 删除光标前一个字符；行首时并入上一行。
    ///
    /// 输入: 无
    /// 输出: 无返回值；缓冲为空时无操作
    pub fn backspace(&mut self) {
        if self.col == 0 && self.row == 0 {
            return;
        }
        self.push_undo();
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let end = char_byte_idx(line, self.col);
            let start = char_byte_idx(line, self.col - 1);
            line.replace_range(start..end, "");
            self.col -= 1;
        } else {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&cur);
        }
        self.after_yank = false;
    }

    /// 删除光标处字符；行尾时并入下一行。
    ///
    /// 输入: 无
    /// 输出: 无返回值；末尾无操作
    pub fn delete_forward(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col == line_len && self.row + 1 >= self.lines.len() {
            return;
        }
        self.push_undo();
        if self.col < line_len {
            let line = &mut self.lines[self.row];
            let start = char_byte_idx(line, self.col);
            let end = char_byte_idx(line, self.col + 1);
            line.replace_range(start..end, "");
        } else {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
        self.after_yank = false;
    }

    // ---------- 光标移动（不入 undo） ----------

    /// 光标左移（行首回上一行行尾）。
    pub fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    /// 光标右移（行尾进下一行行首）。
    pub fn move_right(&mut self) {
        let len = self.lines[self.row].chars().count();
        if self.col < len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// 光标上移（列 clamp 到目标行长度）。
    pub fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    /// 光标下移（列 clamp 到目标行长度）。
    pub fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            let len = self.lines[self.row].chars().count();
            self.col = self.col.min(len);
        }
    }

    /// 光标到行首。
    pub fn home(&mut self) {
        self.col = 0;
    }

    /// 光标到行尾。
    pub fn end(&mut self) {
        self.col = self.lines[self.row].chars().count();
    }

    /// 词左移（跳过空白与非空白，可跨行到上一行行尾）。
    pub fn word_left(&mut self) {
        loop {
            // 先退过空白
            while self.col > 0 && prev_char(&self.lines[self.row], self.col).is_whitespace() {
                self.col -= 1;
            }
            if self.col > 0 {
                // 再退过词
                while self.col > 0 && !prev_char(&self.lines[self.row], self.col).is_whitespace()
                {
                    self.col -= 1;
                }
                return;
            }
            // 行首：能跨行则移到上一行行尾继续，否则停
            if self.row > 0 {
                self.row -= 1;
                self.col = self.lines[self.row].chars().count();
            } else {
                return;
            }
        }
    }

    /// 词右移（对称于 word_left，可跨行到下一行行首）。
    pub fn word_right(&mut self) {
        loop {
            let len = self.lines[self.row].chars().count();
            // 先进过空白
            while self.col < len && next_char(&self.lines[self.row], self.col).is_whitespace() {
                self.col += 1;
            }
            if self.col < len {
                while self.col < len && !next_char(&self.lines[self.row], self.col).is_whitespace()
                {
                    self.col += 1;
                }
                return;
            }
            if self.row + 1 < self.lines.len() {
                self.row += 1;
                self.col = 0;
            } else {
                return;
            }
        }
    }

    // ---------- kill 族（入 kill-ring + undo） ----------

    /// 删除光标前一个词（ctrl+w / alt+backspace），被删文本入环。
    pub fn kill_word_back(&mut self) {
        let (start_col, killed) = {
            let line = &self.lines[self.row];
            let mut col = self.col;
            while col > 0 && prev_char(line, col).is_whitespace() {
                col -= 1;
            }
            while col > 0 && !prev_char(line, col).is_whitespace() {
                col -= 1;
            }
            (col, slice_between(line, col, self.col))
        };
        if start_col == self.col {
            return;
        }
        self.push_undo();
        let line = &mut self.lines[self.row];
        let s = char_byte_idx(line, start_col);
        let e = char_byte_idx(line, self.col);
        line.replace_range(s..e, "");
        self.col = start_col;
        self.push_kill(killed);
    }

    /// 删除光标后一个词（alt+d），被删文本入环；行尾时连同换行并入下一行。
    pub fn kill_word_forward(&mut self) {
        let row = self.row;
        let line_len = self.lines[row].chars().count();
        let mut col = self.col;
        // 跳过空白
        while col < line_len && next_char(&self.lines[row], col).is_whitespace() {
            col += 1;
        }
        // 跳过词
        while col < line_len && !next_char(&self.lines[row], col).is_whitespace() {
            col += 1;
        }
        let (killed, next_ws_len) = if col < line_len {
            (slice_between(&self.lines[row], self.col, col), 0)
        } else if row + 1 < self.lines.len() {
            // 到行尾：杀掉换行与下一行的前导空白，保留下行内容（对齐 kill-word
            // 到"下一个词首"的语义，而非整行）。
            let next = &self.lines[row + 1];
            let ws = next.chars().take_while(|c| c.is_whitespace()).count();
            (format!("\n{}", &next[..char_byte_idx(next, ws)]), ws)
        } else {
            (slice_between(&self.lines[row], self.col, line_len), 0)
        };
        if killed.is_empty() {
            return;
        }
        self.push_undo();
        if row + 1 < self.lines.len() && next_ws_len > 0 || (col >= line_len && row + 1 < self.lines.len()) {
            // 行尾分支：去掉下一行前导空白并并入当前行
            let mut next = self.lines.remove(row + 1);
            let _ = next.drain(..next_ws_len.min(next.len()));
            let rest = std::mem::take(&mut next);
            let line = &mut self.lines[row];
            let at = char_byte_idx(line, self.col);
            let tail = line.split_off(at);
            line.push_str(&rest);
            line.push_str(&tail);
        } else if col < line_len {
            let line = &mut self.lines[row];
            let s = char_byte_idx(line, self.col);
            let e = char_byte_idx(line, col);
            line.replace_range(s..e, "");
        }
        self.push_kill(killed);
    }

    /// 删除光标至行首（ctrl+u），被删文本入环。
    pub fn kill_to_line_start(&mut self) {
        if self.col == 0 {
            return;
        }
        let killed = slice_between(&self.lines[self.row], 0, self.col);
        self.push_undo();
        let line = &mut self.lines[self.row];
        let e = char_byte_idx(line, self.col);
        line.replace_range(..e, "");
        self.col = 0;
        self.push_kill(killed);
    }

    /// 删除光标至行尾（ctrl+k），被删文本入环；行尾时并入下一行。
    pub fn kill_to_line_end(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col == line_len && self.row + 1 >= self.lines.len() {
            return;
        }
        self.push_undo();
        if self.col < line_len {
            let killed = slice_between(&self.lines[self.row], self.col, line_len);
            let line = &mut self.lines[self.row];
            let s = char_byte_idx(line, self.col);
            line.truncate(s);
            self.push_kill(killed);
        } else {
            // 行尾：kill 换行（并入下一行首）
            let next = self.lines.remove(self.row + 1);
            self.push_kill(format!("\n{next}"));
            let line = &mut self.lines[self.row];
            let s = char_byte_idx(line, self.col);
            line.truncate(s);
        }
        self.after_yank = false;
    }

    /// 粘贴 kill-ring 顶部到光标处（ctrl+y）。
    pub fn yank(&mut self) {
        let Some(text) = self.kill_ring.get(self.yank_pos).cloned() else {
            return;
        };
        self.push_undo();
        self.insert_text(&text);
        self.after_yank = true;
    }

    /// 用上一次 kill 的内容替换刚粘贴的内容（alt+y，yank-pop）。
    ///
    /// 输入: 无
    /// 输出: 无返回值；仅紧跟 yank 时生效，环内还有更旧 kill 才回退
    pub fn yank_pop(&mut self) {
        if !self.after_yank || self.yank_pos + 1 >= self.kill_ring.len() {
            return;
        }
        // 撤销上一次粘贴，再粘贴更旧的 kill。
        self.undo();
        self.yank_pos += 1;
        let Some(text) = self.kill_ring.get(self.yank_pos).cloned() else {
            return;
        };
        self.insert_text(&text);
        self.after_yank = true;
    }

    /// 撤销最近一次内容编辑（ctrl+-）；无历史时无操作。
    ///
    /// 输入: 无
    /// 输出: 无返回值；恢复到上一快照（内容+光标）
    pub fn undo(&mut self) {
        if let Some((lines, row, col)) = self.undo.pop() {
            self.lines = lines;
            self.row = row.min(self.lines.len() - 1);
            self.col = col.min(self.lines[self.row].chars().count());
            self.after_yank = false;
        }
    }

    // ---------- 内部 ----------

    /// 压入内容快照（超出上限丢弃最旧）。
    fn push_undo(&mut self) {
        self.undo.push((self.lines.clone(), self.row, self.col));
        if self.undo.len() > UNDO_CAP {
            self.undo.remove(0);
        }
    }

    /// 被 kill 文本入环（新在前），重置 yank 游标。
    fn push_kill(&mut self, killed: String) {
        if killed.is_empty() {
            return;
        }
        self.kill_ring.push_front(killed);
        if self.kill_ring.len() > KILL_RING_CAP {
            self.kill_ring.pop_back();
        }
        self.yank_pos = 0;
        self.after_yank = false;
    }

    /// 无快照的单字插入（insert 快照后调用）。
    fn insert_char(&mut self, c: char) {
        if self.row >= self.lines.len() {
            self.lines.push(String::new());
        }
        let line = &mut self.lines[self.row];
        let byte = char_byte_idx(line, self.col);
        line.insert(byte, c);
        self.col += 1;
    }

    /// 无快照的多行文本插入（yank/内部用），光标停在插入末尾。
    fn insert_text(&mut self, text: &str) {
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        self.insert_str_at_cursor(first);
        for part in parts {
            // 换行 + 后续行
            let byte = char_byte_idx(&self.lines[self.row], self.col);
            let rest = self.lines[self.row].split_off(byte);
            self.lines.insert(self.row + 1, rest);
            self.row += 1;
            self.col = 0;
            self.insert_str_at_cursor(part);
        }
    }

    /// 当前行光标处插入一段（无换行）字符串。
    fn insert_str_at_cursor(&mut self, s: &str) {
        if self.row >= self.lines.len() {
            self.lines.push(String::new());
        }
        let line = &mut self.lines[self.row];
        let byte = char_byte_idx(line, self.col);
        line.insert_str(byte, s);
        self.col += s.chars().count();
    }
}

/// 取光标前一个字符（col 为光标字符序号）。
fn prev_char(line: &str, col: usize) -> char {
    line.chars().nth(col - 1).unwrap_or(' ')
}

/// 取光标处字符（col 为光标字符序号）。
fn next_char(line: &str, col: usize) -> char {
    line.chars().nth(col).unwrap_or(' ')
}

/// 截取 [start, end) 字符区间的子串。
fn slice_between(line: &str, start: usize, end: usize) -> String {
    line.chars().skip(start).take(end.saturating_sub(start)).collect()
}

/// 字符索引 → 字节偏移（char 安全，越界取末尾）。
fn char_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷构造：文本 + 光标到末尾。
    fn buf_at_end(text: &str) -> EditorBuffer {
        let mut b = EditorBuffer::new();
        b.replace(text);
        b
    }

    // ===== 核心（任务 1.1）=====

    #[test]
    fn insert_newline_backspace_text() {
        let mut b = EditorBuffer::new();
        for c in "abc".chars() {
            b.insert(c);
        }
        assert_eq!(b.text(), "abc");
        assert_eq!((b.row(), b.col()), (0, 3));
        b.newline();
        for c in "de".chars() {
            b.insert(c);
        }
        assert_eq!(b.text(), "abc\nde");
        assert_eq!((b.row(), b.col()), (1, 2));
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "abc\n");
        b.backspace(); // 行首回并
        assert_eq!(b.text(), "abc");
        assert_eq!((b.row(), b.col()), (0, 3));
        b.backspace();
        b.backspace();
        b.backspace();
        assert!(b.is_empty());
        b.backspace(); // 空缓冲 no-op
        assert!(b.is_empty());
    }

    #[test]
    fn replace_and_append_block() {
        let mut b = EditorBuffer::new();
        b.replace("line1\nline2");
        assert_eq!(b.text(), "line1\nline2");
        assert_eq!((b.row(), b.col()), (1, 5));
        b.append_block("line3\nline4");
        assert_eq!(b.text(), "line1\nline2\nline3\nline4");
        assert_eq!(b.row(), 3);
        // 空缓冲 append 等价 replace
        let mut empty = EditorBuffer::new();
        empty.append_block("x\ny");
        assert_eq!(empty.text(), "x\ny");
        // 空文本 replace → 单空行
        empty.replace("");
        assert!(empty.is_empty());
        assert_eq!(empty.lines().len(), 1);
    }

    #[test]
    fn cjk_char_safety() {
        let mut b = EditorBuffer::new();
        for c in "中文测试".chars() {
            b.insert(c);
        }
        assert_eq!(b.text(), "中文测试");
        b.backspace();
        assert_eq!(b.text(), "中文测");
        b.home();
        b.delete_forward();
        assert_eq!(b.text(), "文测");
        b.move_end_helper();
        assert_eq!(b.col(), 2, "文测 为 2 字符");
    }

    // ===== 移动（任务 2.1/2.2）=====

    impl EditorBuffer {
        fn move_end_helper(&mut self) {
            self.end();
        }
    }

    #[test]
    fn cursor_movement_boundaries() {
        let mut b = buf_at_end("abc\nde");
        assert_eq!((b.row(), b.col()), (1, 2));
        b.move_right(); // 行尾 → 下行不存在，停
        assert_eq!((b.row(), b.col()), (1, 2));
        b.move_left();
        assert_eq!((b.row(), b.col()), (1, 1));
        b.move_up(); // 到行0，col 保持 1（min(1,3)）
        assert_eq!((b.row(), b.col()), (0, 1));
        b.move_up(); // 顶行停
        assert_eq!(b.row(), 0);
        b.home();
        assert_eq!(b.col(), 0);
        b.move_left(); // 行首无上行 → 停
        assert_eq!((b.row(), b.col()), (0, 0));
        b.move_down();
        assert_eq!(b.row(), 1);
        b.end();
        assert_eq!(b.col(), 2);
        b.move_right(); // 末尾停
        assert_eq!((b.row(), b.col()), (1, 2));
        // 跨行左右
        let mut c = buf_at_end("ab\ncd");
        c.home();
        c.move_left(); // 行首 → 上一行行尾
        assert_eq!((c.row(), c.col()), (0, 2));
        c.move_right(); // 行尾 → 下一行行首
        assert_eq!((c.row(), c.col()), (1, 0));
    }

    #[test]
    fn word_movement_crosses_lines() {
        let mut b = buf_at_end("foo bar\nbaz qux");
        // 从末尾 (1,7) 词左移 → 到 "qux" 起点 (1,4)
        b.word_left();
        assert_eq!((b.row(), b.col()), (1, 4));
        // 再左 → "baz" 起点 (1,0)
        b.word_left();
        assert_eq!((b.row(), b.col()), (1, 0));
        // 跨行到上一行 "bar" 起点 (0,4)
        b.word_left();
        assert_eq!((b.row(), b.col()), (0, 4));
        // 词右移对称
        b.word_right();
        assert_eq!((b.row(), b.col()), (0, 7));
        b.word_right(); // 跨行到 "baz" 后 (1,3)
        assert_eq!((b.row(), b.col()), (1, 3));
        b.word_right(); // 到 "qux" 后 = 末尾
        assert_eq!((b.row(), b.col()), (1, 7));
    }

    #[test]
    fn word_movement_with_cjk_and_spaces() {
        // 字符序: 0,1 空格 2,3 中文 4 空格 5,6 词组 7,8 空格
        let mut b = buf_at_end("  中文 词组  ");
        b.word_left(); // 跳过尾部空白 → 词组起点 5
        assert_eq!((b.row(), b.col()), (0, 5));
        b.word_left(); // 跨空白 → 中文起点 2
        assert_eq!((b.row(), b.col()), (0, 2));
        b.word_right(); // Emacs forward-word：到当前词末（中文 end = 4）
        assert_eq!(b.col(), 4);
    }

    // ===== 删除族（任务 3.1/3.2）=====

    #[test]
    fn delete_forward_and_word_deletes() {
        let mut b = buf_at_end("foo bar");
        b.home();
        b.delete_forward(); // 删 'f'
        assert_eq!(b.text(), "oo bar");
        // 行尾 delete_forward 并入下一行
        let mut c = buf_at_end("ab\ncd");
        c.move_up();
        c.end();
        c.delete_forward();
        assert_eq!(c.text(), "abcd");
        assert_eq!((c.row(), c.col()), (0, 2));

        // ctrl+w：删光标前一个词
        let mut d = buf_at_end("foo bar");
        d.kill_word_back();
        assert_eq!(d.text(), "foo ");
        d.kill_word_back(); // 再删 "foo "（含空白）
        assert_eq!(d.text(), "");
        // 行首 kill_word_back 无操作
        d.kill_word_back();
        assert!(d.is_empty());

        // alt+d：删光标后一个词
        let mut e = buf_at_end("foo bar");
        e.home();
        e.kill_word_forward();
        assert_eq!(e.text(), " bar");
        // 行尾 kill_word_forward 并入下一行
        let mut f = buf_at_end("ab\ncd");
        f.move_up();
        f.end();
        f.kill_word_forward();
        assert_eq!(f.text(), "abcd");
        // 末行末尾无操作
        let mut g = buf_at_end("ab");
        g.end();
        g.kill_word_forward();
        assert_eq!(g.text(), "ab");
    }

    #[test]
    fn kill_to_line_start_and_end() {
        let mut b = buf_at_end("hello world");
        b.home();
        for _ in 0..6 {
            b.move_right();
        } // col=6（"hello " 后）
        b.kill_to_line_start();
        assert_eq!(b.text(), "world");
        assert_eq!(b.col(), 0);
        b.kill_to_line_start(); // 行首无操作
        assert_eq!(b.text(), "world");

        let mut c = buf_at_end("hello world");
        c.home();
        for _ in 0..6 {
            c.move_right();
        }
        c.kill_to_line_end();
        assert_eq!(c.text(), "hello ");
        // 行尾 kill → 并入下一行
        let mut d = buf_at_end("ab\ncd");
        d.move_up();
        d.end();
        d.kill_to_line_end();
        assert_eq!(d.text(), "ab");
        assert_eq!((d.row(), d.col()), (0, 2));
        // 末行末尾无操作
        let mut e = buf_at_end("ab");
        e.end();
        e.kill_to_line_end();
        assert_eq!(e.text(), "ab");
    }

    // ===== kill-ring / yank / undo（任务 4.1/4.2）=====

    #[test]
    fn kill_then_yank_restores() {
        let mut b = buf_at_end("hello world");
        b.home();
        for _ in 0..6 {
            b.move_right();
        }
        b.kill_to_line_end(); // kill "world"
        assert_eq!(b.text(), "hello ");
        b.yank();
        assert_eq!(b.text(), "hello world");
        assert_eq!(b.col(), 11);
    }

    #[test]
    fn yank_pop_cycles_to_older_kill() {
        let mut b = EditorBuffer::new();
        for c in "K1K2".chars() {
            b.insert(c);
        }
        // 两次 kill：先删 K2（行尾区），再删 K1 → ring=[K1, K2]（K1 最新）
        b.end();
        b.kill_to_line_start(); // 删 "K1K2" → ring[0]=K1K2
        b.insert('A');
        b.insert('B');
        b.end();
        b.kill_to_line_start(); // 删 "AB" → ring=[AB, K1K2]
        b.yank(); // 粘贴 AB（最新）
        assert_eq!(b.text(), "AB");
        b.yank_pop(); // 替换为 K1K2
        assert_eq!(b.text(), "K1K2");
        // 环到底再 pop 无变化
        b.yank_pop();
        assert_eq!(b.text(), "K1K2");
        // 空环 yank 无操作
        let mut fresh = EditorBuffer::new();
        fresh.yank();
        assert!(fresh.is_empty());
    }

    #[test]
    fn undo_reverts_last_edit_and_is_bounded() {
        let mut b = EditorBuffer::new();
        for c in "abc".chars() {
            b.insert(c);
        }
        b.newline();
        b.insert('x');
        assert_eq!(b.text(), "abc\nx");
        b.undo(); // 撤销插入 'x'
        assert_eq!(b.text(), "abc\n");
        b.undo(); // 撤销换行
        assert_eq!(b.text(), "abc");
        assert_eq!(b.col(), 3);
        b.undo(); // 撤销最后一个字符
        assert_eq!(b.text(), "ab");
        // 连续 undo 到底不 panic
        for _ in 0..10 {
            b.undo();
        }
        assert!(b.is_empty());
        b.undo(); // 空栈 no-op
        assert!(b.is_empty());
        // undo 上限：100+ 次编辑后栈不超限（内部 cap，行为仍可撤销最近一次）
        let mut big = EditorBuffer::new();
        for i in 0..150 {
            big.insert(char::from_u32('a' as u32 + (i % 26)).unwrap());
        }
        big.undo();
        assert_eq!(big.text().len(), 149);
    }

    #[test]
    fn undo_restores_cursor_with_content() {
        let mut b = buf_at_end("hello");
        b.home();
        for _ in 0..3 {
            b.move_right();
        } // col=3
        b.insert('X'); // 快照记录 col=3
        assert_eq!(b.text(), "helXlo");
        b.undo();
        assert_eq!(b.text(), "hello");
        assert_eq!(b.col(), 3, "光标随快照恢复");
    }

    #[test]
    fn multiline_yank_and_kill_ring_join() {
        // kill_to_line_end 在行尾 kill 的是 "\n" + 下一行，yank 应还原
        let mut b = buf_at_end("ab\ncd");
        b.move_up();
        b.end();
        b.kill_to_line_end();
        assert_eq!(b.text(), "ab");
        b.yank();
        assert_eq!(b.text(), "ab\ncd");
        assert_eq!((b.row(), b.col()), (1, 2));
    }
}
