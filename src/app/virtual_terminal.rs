//! VirtualBackend：进程内"虚拟终端"后端（design D2 / α 路线）。
//!
//! 把 ratatui 的绘制翻译成 ANSI 字节流写入内存缓冲（复用
//! `CrosstermBackend` 的 diff→CSI/SGR 生成），宿主（GUI）取走字节喂给
//! 自己的 `vt100` 解析器 → GPUI 渲染。不查询真实终端：尺寸与光标位置
//! 全部由宿主设置，因此可在无 tty 的 GUI 进程内运行。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::layout::{Position, Size};

/// 宿主可抽取的 ANSI 字节缓冲（跨线程共享；embed 线程写、GUI 线程取）。
#[derive(Clone, Default)]
pub struct AnsiBuffer(Arc<Mutex<Vec<u8>>>);

impl AnsiBuffer {
    /// 取走当前累积的全部字节并清空缓冲。
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    /// 缓冲当前字节数。
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// 缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Write for AnsiBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// ratatui `Backend` 的进程内实现：ANSI 输出进 `AnsiBuffer`，尺寸/光标
/// 由宿主维护（`size()` 绝不查询真实终端）。
pub struct VirtualBackend {
    inner: CrosstermBackend<AnsiBuffer>,
    buf: AnsiBuffer,
    /// 共享尺寸槽：宿主 resize 写入，`size()` 读取（draw 内 autoresize 生效）。
    size: Arc<Mutex<Size>>,
    cursor: Position,
}

impl VirtualBackend {
    /// 创建指定列/行数的虚拟终端（独享尺寸槽）。
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_shared_size(Arc::new(Mutex::new(Size::new(cols, rows))))
    }

    /// 使用宿主共享的尺寸槽创建（embed：宿主持有同一 Arc，可直接 resize）。
    pub fn with_shared_size(size: Arc<Mutex<Size>>) -> Self {
        let buf = AnsiBuffer::default();
        Self {
            inner: CrosstermBackend::new(buf.clone()),
            buf,
            size,
            cursor: Position::ORIGIN,
        }
    }

    /// 宿主调整终端尺寸（下一次 draw 时 ratatui autoresize 生效）。
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = Size::new(cols, rows);
    }

    /// 取走自上次取走以来累积的 ANSI 字节。
    pub fn take_ansi(&mut self) -> Vec<u8> {
        self.buf.take()
    }

    /// 共享句柄：embed 线程持有 backend 时，另一端可直接 `take`。
    pub fn buffer(&self) -> AnsiBuffer {
        self.buf.clone()
    }
}

impl Backend for VirtualBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        // 不向"真实终端"查询（无 tty 会阻塞/失败），返回宿主维护的位置。
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        self.cursor = position;
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(*self.size.lock().unwrap())
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: *self.size.lock().unwrap(),
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;

    #[test]
    fn draw_produces_ansi_text_and_color() {
        let backend = VirtualBackend::new(30, 8);
        let shared = backend.buffer();
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let p = ratatui::widgets::Paragraph::new("hello")
                .style(ratatui::style::Style::default().fg(ratatui::style::Color::Red));
            frame.render_widget(p, frame.area());
        })
        .unwrap();

        let ansi = shared.take();
        let s = String::from_utf8_lossy(&ansi);
        assert!(s.contains("hello"), "ansi: {s:?}");
        assert!(s.contains('\u{1b}'), "ansi must contain escape bytes: {s:?}");
        // 红色前景：crossterm 输出 38;5;1 或 31 之一
        assert!(
            s.contains("38;5;1") || s.contains("[31m"),
            "ansi must carry red fg: {s:?}"
        );
        // take 后缓冲清空
        assert!(shared.is_empty());
    }

    #[test]
    fn size_is_host_controlled_not_queried() {
        let mut backend = VirtualBackend::new(40, 12);
        assert_eq!(backend.size().unwrap(), Size::new(40, 12));
        backend.set_size(100, 40);
        assert_eq!(backend.size().unwrap(), Size::new(100, 40));
    }

    #[test]
    fn cursor_position_is_host_tracked() {
        let mut backend = VirtualBackend::new(10, 10);
        backend.set_cursor_position(Position::new(3, 4)).unwrap();
        assert_eq!(backend.get_cursor_position().unwrap(), Position::new(3, 4));
        // 设置动作本身也写进 ANSI 流（宿主的 vt100 需要看到光标移动）
        let ansi = backend.take_ansi();
        let s = String::from_utf8_lossy(&ansi);
        assert!(s.contains('\u{1b}'), "ansi: {s:?}");
    }
}
