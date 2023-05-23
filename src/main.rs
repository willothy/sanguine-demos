use ratatui::{
    self as tui,
    backend::Backend,
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Tabs},
    Frame,
};
use sanguine::{
    bridge::{Bridge, BridgeInner},
    error::*,
    event::{Event, KeyCode, Modifiers, UserEvent},
    layout::{Constraint, NodeId, Rect},
    surface::Surface,
    widgets::{Border, Menu, TextBox},
    App, Config, RenderCtx, UpdateCtx, Widget,
};

use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, RwLock},
};

pub struct FileDialog<U> {
    pwd: Arc<RwLock<PathBuf>>,
    dirty: Arc<AtomicBool>,
    menu: Arc<RwLock<Menu<U>>>,
}

impl FileDialog<Message> {
    pub fn new() -> FileDialog<Message> {
        FileDialog {
            pwd: Arc::new(RwLock::new(std::env::current_dir().unwrap())),
            dirty: Arc::new(AtomicBool::new(true)),
            menu: Arc::new(RwLock::new(Menu::new("Files"))),
        }
    }
}

pub enum Message {
    Open(PathBuf),
    Close(NodeId),
}

impl Widget<Message, ()> for FileDialog<Message> {
    fn render<'r>(
        &self,
        cx: &RenderCtx<'r, Message, ()>,
        surface: &mut Surface,
    ) -> Option<Vec<(Rect, Arc<RwLock<dyn Widget<Message, ()>>>)>> {
        Border::from_inner("Files", self.menu.clone()).render(cx, surface)
    }

    fn update<'u>(
        &mut self,
        cx: &mut UpdateCtx<'u, Message, ()>,
        event: Event<Message>,
    ) -> sanguine::error::Result<()> {
        if self.dirty.swap(false, std::sync::atomic::Ordering::SeqCst) == true {
            let mut menu = self.menu.write().unwrap();
            menu.clear();
            let pwd = self.pwd.clone();
            let dirty = self.dirty.clone();
            menu.add_item("..", "", move |_, _, _| {
                let mut pwd = pwd.write().unwrap();
                *pwd = pwd
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                dirty.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            for entry in
                std::fs::read_dir(self.pwd.read().unwrap().as_path()).map_err(Error::external)?
            {
                let entry = entry.map_err(Error::external)?;
                let path = entry.path();
                let buf = path.to_path_buf();
                let name = entry.file_name();
                let pwd = self.pwd.clone();
                let dirty = self.dirty.clone();
                if path.is_file() {
                    let owner = cx.owner;
                    menu.add_item(name.to_string_lossy(), "", move |_, _, tx| {
                        tx.send(UserEvent::User(Message::Open(buf.clone()))).ok();
                        tx.send(UserEvent::User(Message::Close(owner))).ok();
                    });
                } else if path.is_dir() {
                    menu.add_item(name.to_string_lossy(), "", move |_, _, tx| {
                        let mut pwd = pwd.write().unwrap();
                        *pwd = buf.clone();
                        tx.send(UserEvent::Tick).ok();
                        dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                    });
                }
            }
        }
        match &event {
            Event::Key(k) if k.key == KeyCode::Escape || k.key == KeyCode::Char('q') => {
                cx.layout.remove_node(cx.owner);
                return Ok(());
            }
            _ => {}
        }
        self.menu.write().unwrap().update(cx, event)?;
        Ok(())
    }
}

pub struct Buffer {
    file: PathBuf,
    editor: Arc<RwLock<TextBox>>,
}

impl Buffer {
    pub fn new(file: PathBuf) -> Result<Buffer> {
        let text = if !file.exists() {
            String::new()
        } else {
            std::fs::read_to_string(&file).map_err(Error::external)?
        };
        Ok(Buffer {
            file,
            editor: Arc::new(RwLock::new(TextBox::from_str(text))),
        })
    }

    pub fn load(&mut self) -> Result<()> {
        let text = std::fs::read_to_string(&self.file).map_err(Error::external)?;
        self.editor = Arc::new(RwLock::new(TextBox::from_str(text)));
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        std::fs::write(
            &self.file,
            self.editor
                .read()
                .map_err(Error::external)?
                .buffer()
                .read()
                .map_err(Error::external)?
                .join("\n"),
        )
        .map_err(Error::external)
    }
}

impl Widget<Message, ()> for Buffer {
    fn update<'u>(
        &mut self,
        cx: &mut UpdateCtx<'u, Message, ()>,
        event: Event<Message>,
    ) -> sanguine::error::Result<()> {
        self.editor
            .write()
            .map_err(Error::external)?
            .update(cx, event)
    }

    fn cursor(&self) -> Option<(Option<usize>, usize, usize)> {
        <TextBox as Widget<Message, ()>>::cursor(&self.editor.read().as_ref().unwrap())
    }

    fn constraint(&self) -> Constraint {
        <TextBox as Widget<Message, ()>>::constraint(&self.editor.read().as_ref().unwrap())
    }

    fn render<'r>(
        &self,
        _cx: &RenderCtx<'r, Message, ()>,
        surface: &mut Surface,
    ) -> Option<Vec<(Rect, Arc<RwLock<dyn Widget<Message, ()>>>)>> {
        let dims = surface.dimensions();
        Some(vec![(
            Rect {
                x: 0.,
                y: 0.,
                width: dims.0 as f32,
                height: dims.1 as f32,
            },
            Arc::new(RwLock::new(Border::from_inner(
                self.file.to_string_lossy(),
                self.editor.clone(),
            ))),
        )])
    }
}

struct MiniEditor {
    tabs: Vec<(String, Arc<RwLock<Buffer>>)>,
    index: usize,
    tab_layout: tui::layout::Layout,
}

impl MiniEditor {
    fn new() -> MiniEditor {
        MiniEditor {
            tabs: vec![],
            index: 0,
            tab_layout: tui::layout::Layout::default()
                .direction(tui::layout::Direction::Vertical)
                .constraints(
                    [
                        tui::layout::Constraint::Length(3),
                        tui::layout::Constraint::Min(0),
                    ]
                    .as_ref(),
                ),
        }
    }

    fn add_tab(&mut self, title: impl Into<String>, widget: Buffer) {
        self.tabs
            .push((title.into(), Arc::new(RwLock::new(widget))));
    }

    pub fn next(&mut self) {
        self.index = (self.index + 1) % self.tabs.len();
    }

    pub fn previous(&mut self) {
        if self.index > 0 {
            self.index -= 1;
        } else {
            self.index = self.tabs.len() - 1;
        }
    }
}

fn ui<B: Backend, U>(f: &mut Frame<B>, app: &MiniEditor) -> tui::layout::Rect {
    let size = f.size();
    let chunks = app.tab_layout.split(size);

    let block = Block::default().style(Style::default().bg(Color::Reset).fg(Color::White));
    f.render_widget(block, size);
    let titles = app
        .tabs
        .iter()
        .map(|(t, _)| Spans::from(vec![Span::styled(t, Style::default().fg(Color::Yellow))]))
        .collect();
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("Tabs"))
        .select(app.index)
        .style(Style::default().fg(Color::Cyan))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::Black),
        );
    f.render_widget(tabs, chunks[0]);
    chunks[1]
}

impl Widget<Message, ()> for MiniEditor {
    fn render(
        &self,
        _: &RenderCtx<'_, Message, ()>,
        mut surface: &mut Surface,
    ) -> Option<Vec<(Rect, Arc<RwLock<dyn Widget<Message, ()>>>)>> {
        let mut rect = tui::layout::Rect::default();
        surface
            .ratatui()
            .draw(|f: &mut Frame<BridgeInner>| {
                rect = ui::<_, ()>(f, self);
            })
            .unwrap();
        self.tabs.get(self.index).map(|(_, widget)| {
            let w: Arc<RwLock<dyn Widget<Message, ()>>> = widget.clone();
            vec![(
                Rect {
                    x: rect.x as f32,
                    y: rect.y as f32,
                    width: rect.width as f32,
                    height: rect.height as f32,
                },
                w,
            )]
        })
    }

    fn cursor(&self) -> Option<(Option<usize>, usize, usize)> {
        self.tabs
            .get(self.index)
            .map(|(_, widget)| {
                widget
                    .read()
                    .as_ref()
                    .unwrap()
                    .cursor()
                    .map(|(_, x, y)| (Some(2), x, y))
            })
            .flatten()
    }

    fn update<'u>(
        &mut self,
        cx: &mut UpdateCtx<'u, Message, ()>,
        event: Event<Message>,
    ) -> sanguine::error::Result<()> {
        match event {
            Event::Key(k) if k.key == KeyCode::RightArrow && k.modifiers == Modifiers::SHIFT => {
                self.next()
            }
            Event::Key(k) if k.key == KeyCode::LeftArrow && k.modifiers == Modifiers::SHIFT => {
                self.previous()
            }
            Event::Key(k) if k.modifiers == Modifiers::CTRL && k.key == KeyCode::Char('s') => {
                // save file
                if let Some((_, widget)) = self.tabs.get(self.index) {
                    let buffer = widget.write().unwrap();
                    buffer.save()?;
                }
            }
            Event::Mouse(_) => {}
            _ => {
                if let Some((_, widget)) = self.tabs.get(self.index) {
                    widget.write().unwrap().update(cx, event)?;
                }
            }
        }
        Ok(())
    }
}

pub fn main() -> Result<()> {
    let editor = Arc::new(RwLock::new(MiniEditor::new()));
    let mut app = App::<(), Message>::new(
        // The default config is fine for this example
        Config::default(),
    )?
    .with_handler({
        let editor = editor.clone();
        move |this, event, _| {
            match event {
                Event::Key(k) if k.modifiers == Modifiers::CTRL && k.key == KeyCode::Char('o') => {
                    let float = this.update_layout(|l| {
                        l.add_floating(
                            FileDialog::new(),
                            Rect {
                                x: 25.0,
                                y: 20.0,
                                width: 20.,
                                height: 15.,
                            },
                        )
                    });
                    this.set_focus(float)?;
                }
                Event::User(UserEvent::User(Message::Open(file))) => {
                    editor.write().unwrap().add_tab(
                        file.file_name().unwrap().to_string_lossy().to_string(),
                        Buffer::new(file.clone())?,
                    );
                }
                Event::User(UserEvent::User(Message::Close(float))) => {
                    let node = this.update_layout(|l| {
                        l.remove_node(*float);
                        l.leaves().first().copied().unwrap()
                    });
                    this.set_focus(node)?;
                    return Ok(true);
                }
                _ => {}
            }
            Ok(false)
        }
    });
    let main = app.update_layout(move |layout| {
        // Add the first editor to the layout
        let main = layout.add_leaf_raw(editor);

        layout.add_child(layout.root(), main);
        Ok(main)
    })?;
    app.set_focus(main)?;

    while app.handle_events()? {
        app.render()?;
    }

    Ok(())
}
