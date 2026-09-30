use crate::windows::{self, Event};
use native_windows_gui as nwg;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

#[derive(Default)]
struct Ui {
    window: nwg::Window,
    title: nwg::Label,
    detail: nwg::Label,
    progress: nwg::ProgressBar,
    action: nwg::Button,
    cancel: nwg::Button,
    notice: nwg::Notice,
    receiver: RefCell<Option<mpsc::Receiver<Event>>>,
    cancelled: RefCell<Arc<AtomicBool>>,
    applying: Arc<AtomicBool>,
    busy: Cell<bool>,
    installed: RefCell<Option<PathBuf>>,
}

impl Ui {
    fn start(&self) {
        if self.busy.replace(true) {
            return;
        }
        self.action.set_enabled(false);
        self.cancel.set_text("Cancel");
        self.progress.set_pos(0);
        self.detail.set_text("Checking the latest signed release…");
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancelled.borrow_mut() = cancel.clone();
        let applying = self.applying.clone();
        applying.store(false, Ordering::Release);
        let (sender, receiver) = mpsc::channel();
        *self.receiver.borrow_mut() = Some(receiver);
        let notice = self.notice.sender();
        std::thread::spawn(move || {
            let notify = |event| {
                let _ = sender.send(event);
                notice.notice();
            };
            let result = tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    runtime.block_on(windows::install_latest(cancel, applying.clone(), &notify))
                });
            if let Err(error) = result {
                notify(Event::Error(format!("{error}")));
            }
            applying.store(false, Ordering::Release);
        });
    }

    fn events(&self) {
        let receiver = self.receiver.borrow();
        let Some(receiver) = receiver.as_ref() else {
            return;
        };
        while let Ok(event) = receiver.try_recv() {
            match event {
                Event::Status(text) => self.detail.set_text(&text),
                Event::Progress(percent) => self.progress.set_pos(percent),
                Event::Applying => {
                    self.detail
                        .set_text("Installing Axiom… Please wait until installation finishes.");
                    self.cancel.set_enabled(false);
                }
                Event::Complete(path) => {
                    self.busy.set(false);
                    self.title.set_text("Axiom is ready");
                    self.detail.set_text(
                        "The latest Axiom is installed. Your conversations stay on this device.",
                    );
                    self.progress.set_pos(100);
                    *self.installed.borrow_mut() = Some(path);
                    self.action.set_text("Open Axiom");
                    self.action.set_enabled(true);
                    self.cancel.set_text("Close");
                    self.cancel.set_enabled(true);
                }
                Event::Error(error) => {
                    self.busy.set(false);
                    self.detail.set_text(&format!("Couldn’t finish setup. {error}\r\nYou can retry, or download the offline installer from axiom.stream/downloads."));
                    self.action.set_text("Retry");
                    self.action.set_enabled(true);
                    self.cancel.set_text("Close");
                    self.cancel.set_enabled(true);
                }
                Event::Cancelled => {
                    self.busy.set(false);
                    self.detail
                        .set_text("Setup cancelled. The downloaded files have been removed.");
                    self.action.set_text("Install Axiom");
                    self.action.set_enabled(true);
                    self.cancel.set_text("Close");
                }
            }
        }
    }
    fn close(&self) {
        if self.applying.load(Ordering::Acquire) {
            nwg::modal_info_message(
                &self.window.handle,
                "Installing Axiom",
                "Wait for installation to finish before closing setup.",
            );
        } else if self.busy.get() {
            self.cancelled.borrow().store(true, Ordering::Release);
            self.detail.set_text("Cancelling setup…");
        } else {
            nwg::stop_thread_dispatch();
        }
    }
}

pub fn run(smoke: bool) -> anyhow::Result<()> {
    nwg::init()?;
    nwg::Font::set_global_family("Segoe UI")?;
    let mut ui = Ui::default();
    nwg::Window::builder()
        .size((520, 290))
        .center(true)
        .title("Axiom Setup")
        .flags(
            nwg::WindowFlags::WINDOW
                | if smoke {
                    nwg::WindowFlags::empty()
                } else {
                    nwg::WindowFlags::VISIBLE
                },
        )
        .build(&mut ui.window)?;
    nwg::Label::builder()
        .parent(&ui.window)
        .position((28, 28))
        .size((460, 35))
        .text("Install the latest Axiom")
        .build(&mut ui.title)?;
    nwg::Label::builder().parent(&ui.window).position((28,78)).size((460,104))
        .text("Setup downloads the latest version, verifies it and installs Axiom for your Windows PC. An internet connection is required.").build(&mut ui.detail)?;
    nwg::ProgressBar::builder()
        .parent(&ui.window)
        .position((28, 193))
        .size((460, 12))
        .range(0..100)
        .build(&mut ui.progress)?;
    nwg::Button::builder()
        .parent(&ui.window)
        .position((274, 228))
        .size((136, 32))
        .text("Install Axiom")
        .focus(true)
        .build(&mut ui.action)?;
    nwg::Button::builder()
        .parent(&ui.window)
        .position((420, 228))
        .size((68, 32))
        .text("Close")
        .build(&mut ui.cancel)?;
    nwg::Notice::builder()
        .parent(&ui.window)
        .build(&mut ui.notice)?;
    if smoke {
        return Ok(());
    }
    let ui = Rc::new(ui);
    let weak = Rc::downgrade(&ui);
    let handler = nwg::full_bind_event_handler(&ui.window.handle, move |event, _, handle| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match event {
            nwg::Event::OnWindowClose => ui.close(),
            nwg::Event::OnNotice if handle == ui.notice.handle => ui.events(),
            nwg::Event::OnButtonClick if handle == ui.cancel.handle => ui.close(),
            nwg::Event::OnButtonClick if handle == ui.action.handle => {
                if let Some(path) = ui.installed.borrow().as_ref() {
                    match windows::open(path) {
                        Ok(()) => nwg::stop_thread_dispatch(),
                        Err(error) => {
                            nwg::modal_error_message(
                                &ui.window.handle,
                                "Couldn’t open Axiom",
                                &format!("{error}"),
                            );
                        }
                    }
                } else {
                    ui.start();
                }
            }
            _ => {}
        }
    });
    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&handler);
    Ok(())
}
