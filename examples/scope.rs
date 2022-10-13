use atb::AtomicTripleBuffer;
use futures_channel::mpsc::UnboundedReceiver;
use gtk::{gdk, gio, prelude::*};
use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    error::Error,
    ffi::CString,
    rc::Rc,
    sync::{atomic::AtomicBool, Arc},
};
use tokio_stream::StreamExt;

struct Port {
    pub id: jack2::OwnedPortUuid,
    pub name: String,
    pub color: gdk::RGBA,
    pub buffer: Arc<AtomicTripleBuffer<PortBuffer>>,
}

#[derive(Clone, Debug)]
struct PortBuffer {
    data: Vec<f32>,
    nframes: usize,
}

enum ProcessMessage {
    Render,
}

struct ClientState {
    tx: rtrb::Producer<ProcessMessage>,
    recv_thread: Option<std::thread::JoinHandle<()>>,
    quit: Arc<AtomicBool>,
    last_empty: bool,
}

impl ClientState {
    fn new() -> (Self, UnboundedReceiver<ProcessMessage>) {
        let (main_tx, main_rx) = futures_channel::mpsc::unbounded();
        let (tx, mut rx) = rtrb::RingBuffer::new(1024);
        let quit = Arc::new(AtomicBool::new(false));
        let recv_thread = Some({
            let quit = quit.clone();
            std::thread::spawn(move || loop {
                if quit.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                while let Ok(msg) = rx.pop() {
                    main_tx.unbounded_send(msg).ok();
                }
                std::thread::park();
            })
        });
        (
            Self {
                tx,
                recv_thread,
                quit,
                last_empty: false,
            },
            main_rx,
        )
    }
}

impl jack2::ProcessHandler for ClientState {
    type PortData = Arc<AtomicTripleBuffer<PortBuffer>>;
    fn process(
        &mut self,
        scope: jack2::ProcessScope<Self::PortData>,
    ) -> std::ops::ControlFlow<(), ()> {
        let mut empty = true;
        for port in scope.ports() {
            let buf = port.audio_in_buffer();
            let data = port.data();
            let data = data.borrow();
            let mut back = data.back_buffers().unwrap();
            let out = back.back_mut();
            let nframes = buf.len().min(out.data.len());
            out.data[..nframes].copy_from_slice(&buf[..nframes]);
            out.nframes = nframes;
            empty = false;
            back.swap();
        }
        if !empty || self.last_empty != empty {
            self.tx.push(ProcessMessage::Render).ok();
            self.recv_thread.as_ref().unwrap().thread().unpark();
        }
        self.last_empty = empty;
        std::ops::ControlFlow::Continue(())
    }
}

impl Drop for ClientState {
    fn drop(&mut self) {
        self.quit.store(true, std::sync::atomic::Ordering::Release);
        self.recv_thread.take().unwrap().join().unwrap();
    }
}

struct Scope {
    client: jack2::ActiveClient<jack2::glib::GlibContext, ClientState, ()>,
    ports: RefCell<indexmap::IndexMap<jack2::Uuid, Port>>,
    port_counter: Cell<u64>,
    color_counter: Cell<usize>,
}

impl Scope {
    #[inline]
    pub fn new(state: ClientState) -> Result<Self, Box<dyn Error>> {
        let (client, _) = jack2::ClientBuilder::new("rust-jack2-scope")
            .flags(jack2::ClientOptions::NO_START_SERVER)
            .build()?;
        let client = client.activate(Some(state), None::<()>, jack2::ActivateFlags::empty())?;
        let scope = Self {
            client,
            ports: Default::default(),
            port_counter: Cell::new(2),
            color_counter: Cell::new(2),
        };
        scope.add_port("Port 1", gdk::RGBA::new(0.5, 0.5, 0.5, 0.8))?;
        Ok(scope)
    }
    #[inline]
    pub fn add_port(
        &self,
        name: impl Into<String>,
        color: gdk::RGBA,
    ) -> Result<jack2::Uuid, Box<dyn Error>> {
        let bufsize = (self.client.buffer_size() as usize)
            .next_power_of_two()
            .max(8192);
        let buffer = Arc::new(AtomicTripleBuffer::new(PortBuffer {
            data: vec![0.0; bufsize],
            nframes: 0,
        }));
        let mut name = name.into();
        if name.is_empty() {
            let count = self.port_counter.get();
            name = format!("Port {}", count);
            self.port_counter.set(count + 1);
        }
        let id = self
            .client
            .register_ports([jack2::PortInfo {
                name: &name,
                type_: jack2::PortType::Audio,
                mode: jack2::PortMode::Input,
                flags: jack2::PortCreateFlags::TERMINAL,
                data: buffer.clone(),
            }])
            .remove(0)?;
        let mut ports = self.ports.borrow_mut();
        let uuid = id.uuid();
        ports.insert(
            uuid,
            Port {
                id,
                name,
                color,
                buffer,
            },
        );
        Ok(uuid)
    }
    #[inline]
    pub fn port(&self, id: jack2::Uuid) -> Option<Ref<Port>> {
        Ref::filter_map(self.ports.borrow(), |ps| ps.get(&id)).ok()
    }
    #[inline]
    pub fn port_mut(&self, id: jack2::Uuid) -> Option<RefMut<Port>> {
        RefMut::filter_map(self.ports.borrow_mut(), |ps| ps.get_mut(&id)).ok()
    }
    #[inline]
    pub fn for_each_port(&self, func: impl FnMut(&Port)) {
        self.ports.borrow().values().for_each(func)
    }
    #[inline]
    pub fn remove_port(&self, id: jack2::Uuid) {
        let id = self.ports.borrow_mut().remove(&id).unwrap().id;
        self.client.unregister_ports([id]);
    }
    #[inline]
    pub fn render(
        &self,
        canvas: &mut femtovg::Canvas<femtovg::renderer::OpenGl>,
        (width, height): (u32, u32),
        vrange: f32,
    ) {
        use femtovg::{Color, Paint, Path};
        use realfft::num_complex::Complex;

        #[inline]
        fn to_fvg(c: &gdk::RGBA) -> Color {
            Color::rgbaf(c.red(), c.green(), c.blue(), c.alpha())
        }

        let mut planner = realfft::RealFftPlanner::new();
        let bufsize = (self.client.buffer_size() as usize).next_power_of_two();
        let mut output = vec![Complex::default(); bufsize];
        let mut scratch = vec![Complex::default(); bufsize];
        let ports = self.ports.borrow();

        canvas.clear_rect(0, 0, width, height, Color::rgbaf(0.0, 0.0, 0.0, 0.0));
        let width = width as f32;
        let height = height as f32;
        for port in ports.values() {
            let mut buf = port.buffer.front_buffer().unwrap();
            let nframes = buf.nframes;
            let pl = planner.plan_fft_forward(nframes);
            let scratch_len = pl.get_scratch_len();
            if scratch.len() < scratch_len {
                scratch.resize(scratch_len, Complex::default());
            }
            let outframes = nframes / 2 + 1;
            if output.len() < outframes {
                output.resize(outframes, Complex::default());
            }
            pl.process_with_scratch(
                &mut buf.data[..nframes],
                &mut output[..outframes],
                &mut scratch[..scratch_len],
            )
            .unwrap();
            let mut path = Path::new();
            path.move_to(0.5, height + 0.5);
            let imax = width / outframes as f32;
            for i in 0..outframes {
                let mag = 1.0 - unsafe { output.get_unchecked(i) }.norm_sqr().sqrt() / vrange;
                path.line_to((i as f32).mul_add(imax, 0.5), mag.mul_add(height, 0.5));
            }
            path.line_to(width + 0.5, height + 0.5);
            path.close();
            let paint = Paint::color(to_fvg(&port.color));
            canvas.fill_path(&mut path, paint);
            if buf.data.len() < bufsize {
                buf.data.resize(bufsize, 0.0);
            }
        }
        canvas.flush();
    }
}

fn create_port_dialog(
    scope: glib::BoxedAnyObject,
    model: gio::ListStore,
    parent: &gtk::Window,
    edit: Option<(jack2::Uuid, gtk::ListItem)>,
) {
    let builder = gtk::Builder::new();
    let builder_scope = gtk::BuilderRustScope::new();
    builder_scope.add_callback("hide_error", |values| {
        values[0]
            .get::<&gtk::Revealer>()
            .unwrap()
            .set_reveal_child(false);
        None
    });
    builder.set_scope(Some(&builder_scope));
    builder.add_from_string(ADD_PORT_DIALOG_XML).unwrap();
    let dialog = builder.object::<gtk::Dialog>("add-port-dialog").unwrap();
    let input = builder.object::<gtk::Entry>("port-name").unwrap();
    let color = builder.object::<gtk::ColorButton>("port-color").unwrap();
    let error_revealer = builder.object::<gtk::Revealer>("error-revealer").unwrap();
    let error_label = builder.object::<gtk::Label>("error-label").unwrap();
    let ok_button = builder.object::<gtk::Button>("ok-response").unwrap();

    dialog.set_transient_for(Some(parent));
    dialog.set_application(parent.application().as_ref());
    input.set_max_length(jack2::client_name_size().unwrap() as i32);

    let colors = std::array::from_fn::<_, 50, _>(|c| {
        let y = (c % 5) as f32 / 5.0;
        let x = c / 5;
        if x == 9 {
            let y = y.mul_add(0.5, 0.4);
            gdk::RGBA::new(y, y, y, 0.8)
        } else {
            let x = x as f32 / 9.0;
            let (r, g, b) = gtk::hsv_to_rgb(x, 0.8, y.mul_add(0.6, 0.2));
            gdk::RGBA::new(r, g, b, 0.8)
        }
    });
    color.add_palette(gtk::Orientation::Vertical, 5, colors.as_slice());

    if let Some((id, _)) = &edit {
        let scope = scope.borrow::<Scope>();
        let port = scope.port(*id).unwrap();
        dialog.set_title(Some("Edit Port"));
        ok_button.set_label("Save");
        input.set_text(&port.name);
        color.set_rgba(&port.color);
    } else {
        let color_index = scope.borrow::<Scope>().color_counter.get();
        color.set_rgba(&colors[color_index % colors.len()]);
    }

    let color_changed = Rc::new(Cell::new(false));
    {
        let dialog = dialog.downgrade();
        input.connect_activate(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.response(gtk::ResponseType::Ok);
            }
        });
    }
    {
        let color_changed = color_changed.clone();
        color.connect_rgba_notify(move |_| {
            color_changed.set(true);
        });
    }
    let parent = parent.downgrade();
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Ok {
            dialog.destroy();
            return;
        }
        if let Some((id, item)) = &edit {
            let scope_ = scope.borrow::<Scope>();
            let mut port = scope_.port_mut(*id).unwrap();
            let name = String::from(input.text());
            if name != port.name {
                if let Err(e) = scope_
                    .client
                    .port_rename(port.id, CString::new(name.clone()).unwrap())
                {
                    error_label.set_text(&e.to_string());
                    error_revealer.set_reveal_child(true);
                    return;
                }
                port.name = name;
                update_item(item, &port.name);
            }
            let rgba = color.rgba();
            if rgba != port.color {
                port.color = rgba;
                if let Some(parent) = parent.upgrade() {
                    parent.queue_draw();
                }
            }
            dialog.destroy();
        } else {
            let scope = scope.borrow::<Scope>();
            match scope.add_port(input.text(), color.rgba()) {
                Ok(id) => {
                    if !color_changed.get() {
                        scope
                            .color_counter
                            .set(scope.color_counter.get().wrapping_add(7));
                    }
                    model.append(&glib::BoxedAnyObject::new(id));
                    dialog.destroy();
                }
                Err(e) => {
                    error_label.set_text(&e.to_string());
                    error_revealer.set_reveal_child(true);
                }
            }
        }
    });

    dialog.show();
}

fn update_item(item: &gtk::ListItem, name: &str) {
    let label = item
        .child()
        .unwrap()
        .first_child()
        .unwrap()
        .next_sibling()
        .unwrap()
        .downcast::<gtk::Label>()
        .unwrap();
    label.set_text(name);
}

fn create_ui(scope: Scope, mut rx: UnboundedReceiver<ProcessMessage>, app: &gtk::Application) {
    let model = gio::ListStore::new(glib::BoxedAnyObject::static_type());
    scope.for_each_port(|port| model.append(&glib::BoxedAnyObject::new(port.id.uuid())));
    let scope = glib::BoxedAnyObject::new(scope);

    let builder = gtk::Builder::from_string(MAIN_WINDOW_XML);
    let window = builder.object::<gtk::Window>("main").unwrap();
    let add_port = builder.object::<gtk::Button>("add-port").unwrap();
    let vertical_range = builder.object::<gtk::SpinButton>("vertical-range").unwrap();
    let list = builder.object::<gtk::ListView>("port-list").unwrap();
    let selection = builder.object::<gtk::NoSelection>("selection").unwrap();
    let area = builder.object::<gtk::GLArea>("canvas").unwrap();

    window.set_application(Some(app));

    {
        let scope = scope.clone();
        let model = model.clone();
        let window = window.clone();
        add_port.connect_clicked(move |_| {
            create_port_dialog(scope.clone(), model.clone(), &window, None);
        });
    }

    {
        let area = area.downgrade();
        vertical_range.connect_value_changed(move |_| {
            if let Some(area) = area.upgrade() {
                area.queue_render();
            }
        });
    }
    {
        let area = area.downgrade();
        model.connect_items_changed(move |_, _, _, _| {
            if let Some(area) = area.upgrade() {
                area.queue_render();
            }
        });
    }

    let factory = gtk::SignalListItemFactory::new();
    {
        let scope = scope.clone();
        let model = model.clone();
        let window = window.clone();
        factory.connect_setup(move |_, item| {
            let builder = gtk::Builder::from_string(LIST_ITEM_XML);
            let row = builder.object::<gtk::Box>("row").unwrap();
            let edit = builder.object::<gtk::Button>("edit-port").unwrap();
            let remove = builder.object::<gtk::Button>("remove-port").unwrap();

            item.set_activatable(false);
            item.set_selectable(false);
            item.set_child(Some(&row));

            {
                let item = item.clone();
                let scope = scope.clone();
                let model = model.clone();
                let window = window.clone();
                edit.connect_clicked(move |_| {
                    let id = *item
                        .item()
                        .unwrap()
                        .downcast::<glib::BoxedAnyObject>()
                        .unwrap()
                        .borrow();
                    create_port_dialog(
                        scope.clone(),
                        model.clone(),
                        &window,
                        Some((id, item.clone())),
                    );
                });
            }

            let item = item.clone();
            let scope = scope.clone();
            let model = model.clone();
            remove.connect_clicked(move |_| {
                let id = *item
                    .item()
                    .unwrap()
                    .downcast::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow();
                scope.borrow::<Scope>().remove_port(id);
                model.remove(item.position());
            });
        });
    }
    {
        let scope = scope.clone();
        factory.connect_bind(move |_, item| {
            let id = *item
                .item()
                .unwrap()
                .downcast::<glib::BoxedAnyObject>()
                .unwrap()
                .borrow();
            let scope = scope.borrow::<Scope>();
            let port = scope.port(id).unwrap();
            update_item(item, &port.name);
        });
    }

    list.set_factory(Some(&factory));
    selection.set_model(Some(&model));

    let canvas = Rc::new(RefCell::new(None));
    {
        let canvas = canvas.clone();
        area.connect_render(move |area, _| {
            let w = area.width() as u32;
            let h = area.height() as u32;
            let vrange = vertical_range.value() as f32;
            scope
                .borrow::<Scope>()
                .render(canvas.borrow_mut().as_mut().unwrap(), (w, h), vrange);
            glib::signal::Inhibit(false)
        });
    }
    {
        let canvas = canvas.clone();
        area.connect_unrealize(move |_| {
            canvas.take();
        });
    }
    area.connect_resize(move |area, w, h| {
        canvas
            .borrow_mut()
            .get_or_insert_with(|| {
                use femtovg::{renderer, Canvas};
                use glow::HasContext;

                area.attach_buffers();

                static LOAD_FN: fn(&str) -> *const std::ffi::c_void =
                    |s| epoxy::get_proc_addr(s) as *const _;
                let mut renderer = unsafe {
                    renderer::OpenGl::new_from_function(LOAD_FN).expect("Cannot create renderer")
                };

                let fbo_id = unsafe {
                    let ctx = glow::Context::from_loader_function(LOAD_FN);
                    let id = ctx.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32;
                    assert!(id != 0);
                    ctx.bind_framebuffer(glow::FRAMEBUFFER, None);
                    // TODO - transmute can be removed when a new version of glow is released with this
                    // patch: https://github.com/grovesNL/glow/pull/211
                    std::mem::transmute(id)
                };
                renderer.set_screen_target(Some(fbo_id));
                Canvas::new(renderer).expect("Cannot create canvas")
            })
            .set_size(w as u32, h as u32, area.scale_factor() as f32);
    });

    let area = area.downgrade();
    glib::MainContext::default().spawn_local(async move {
        while let Some(_msg) = rx.next().await {
            if let Some(area) = area.upgrade() {
                area.queue_render();
            }
        }
    });

    window.show();
}

fn activate(app: &gtk::Application) {
    if !app.windows().is_empty() {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_data(STYLES.as_bytes());
    gtk::StyleContext::add_provider_for_display(
        &gdk::Display::default().unwrap(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let (client_state, rx) = ClientState::new();
    match Scope::new(client_state) {
        Ok(scope) => create_ui(scope, rx, app),
        Err(e) => {
            let dialog = gtk::MessageDialog::builder()
                .application(app)
                .text(&e.to_string())
                .build();
            dialog.add_button("Close", gtk::ResponseType::Close);
            dialog.connect_response(|dialog, _| dialog.destroy());
            dialog.show();
        }
    }
}

fn main() {
    init_epoxy();
    let app = gtk::Application::new(
        Some("io.github.jf2048.rust-jack2.examples.Scope"),
        gio::ApplicationFlags::FLAGS_NONE,
    );
    app.connect_activate(activate);
    app.run();
}

pub fn init_epoxy() {
    #[cfg(target_os = "macos")]
    let library = unsafe { libloading::os::unix::Library::new("libepoxy.0.dylib") }.unwrap();
    #[cfg(all(unix, not(target_os = "macos")))]
    let library = unsafe { libloading::os::unix::Library::new("libepoxy.so.0") }.unwrap();
    #[cfg(windows)]
    let library = libloading::os::windows::Library::open_already_loaded("libepoxy-0.dll").unwrap();

    epoxy::load_with(|name| {
        unsafe { library.get::<_>(name.as_bytes()) }
            .map(|symbol| *symbol)
            .unwrap_or(std::ptr::null())
    });
}

const MAIN_WINDOW_XML: &str = r#"
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <requires lib="gtk" version="4.0"/>
    <object class="GtkApplicationWindow" id="main">
    <property name="title" translatable="true">Scope</property>
    <property name="default-width">500</property>
    <property name="default-height">350</property>
    <child type="titlebar">
      <object class="GtkHeaderBar">
        <child type="start">
          <object class="GtkSpinButton" id="vertical-range">
            <property name="digits">1</property>
            <property name="width-chars">5</property>
            <property name="adjustment">
              <object class="GtkAdjustment">
                <property name="lower">1</property>
                <property name="upper">1024</property>
                <property name="step-increment">1</property>
                <property name="page-increment">8</property>
                <property name="value">32</property>
              </object>
            </property>
            <property name="tooltip-text" translatable="true">Vertical Range</property>
          </object>
        </child>
        <child type="end">
          <object class="GtkToggleButton" id="show-port-list">
            <property name="icon-name">view-list-bullet-symbolic</property>
            <property name="tooltip-text" translatable="true">Show Port List</property>
          </object>
        </child>
        <child type="end">
          <object class="GtkButton" id="add-port">
            <property name="icon-name">list-add-symbolic</property>
            <property name="tooltip-text" translatable="true">Add Port</property>
          </object>
        </child>
      </object>
    </child>
    <child>
      <object class="GtkPaned">
        <property name="position">300</property>
        <child type="start">
          <object class="GtkGLArea" id="canvas">
            <style>
              <class name="scope"/>
            </style>
            <property name="width-request">20</property>
            <property name="height-request">20</property>
            <property name="has-stencil-buffer">1</property>
          </object>
        </child>
        <child type="end">
          <object class="GtkScrolledWindow">
            <property name="visible" bind-source="show-port-list" bind-property="active">0</property>
            <property name="hscrollbar-policy">never</property>
            <child>
              <object class="GtkListView" id="port-list">
                <property name="show-separators">1</property>
                <property name="model">
                  <object class="GtkNoSelection" id="selection"></object>
                </property>
              </object>
            </child>
          </object>
        </child>
        <property name="resize-start-child">1</property>
        <property name="resize-end-child">0</property>
      </object>
    </child>
  </object>
</interface>
"#;

const ADD_PORT_DIALOG_XML: &str = r#"
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <requires lib="gtk" version="4.0"/>
  <object class="GtkDialog" id="add-port-dialog">
    <property name="title" translatable="true">Add Port</property>
    <property name="use-header-bar">1</property>
    <property name="modal">1</property>
    <child type="action">
      <object class="GtkButton" id="ok-response">
        <property name="label" translatable="true">Add</property>
      </object>
    </child>
    <child type="action">
      <object class="GtkButton" id="cancel-response">
        <property name="label" translatable="true">Cancel</property>
      </object>
    </child>
    <child>
      <object class="GtkBox" id="welcome">
        <property name="orientation">vertical</property>
        <child>
          <object class="GtkRevealer" id="error-revealer">
            <property name="reveal-child">0</property>
            <child>
              <object class="GtkInfoBar">
                <property name="message-type">error</property>
                <property name="show-close-button">1</property>
                <signal name="close" handler="hide_error" swapped="true" object="error-revealer"/>
                <signal name="response" handler="hide_error" swapped="true" object="error-revealer"/>
                <child>
                  <object class="GtkLabel" id="error-label">
                    <property name="selectable">1</property>
                  </object>
                </child>
              </object>
            </child>
          </object>
        </child>
        <child>
          <object class="GtkBox">
            <style>
              <class name="add-port-form"/>
            </style>
            <child>
              <object class="GtkEntry" id="port-name">
                <property name="hexpand">1</property>
                <property name="placeholder-text" translatable="true">Port Name</property>
                <property name="tooltip-text" translatable="true">Port Name</property>
                <signal name="notify::text" handler="hide_error" swapped="true" object="error-revealer"/>
              </object>
            </child>
            <child>
              <object class="GtkColorButton" id="port-color">
                <property name="tooltip-text" translatable="true">Port Color</property>
                <property name="use-alpha">1</property>
              </object>
            </child>
          </object>
        </child>
      </object>
    </child>
    <action-widgets>
      <action-widget response="ok">ok-response</action-widget>
      <action-widget response="cancel">cancel-response</action-widget>
    </action-widgets>
  </object>
</interface>
"#;

const LIST_ITEM_XML: &str = r#"
<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <requires lib="gtk" version="4.0"/>
  <object class="GtkBox" id="row">
    <style>
      <class name="port-row"/>
    </style>
    <child>
      <object class="GtkButton" id="edit-port">
        <style>
          <class name="flat"/>
        </style>
        <property name="icon-name">document-edit-symbolic</property>
        <property name="tooltip-text" translatable="true">Edit Port</property>
      </object>
    </child>
    <child>
      <object class="GtkLabel" id="port-name">
        <property name="hexpand">1</property>
        <property name="xalign">0</property>
        <property name="ellipsize">end</property>
      </object>
    </child>
    <child>
      <object class="GtkButton" id="remove-port">
        <style>
          <class name="flat"/>
        </style>
        <property name="icon-name">edit-delete-symbolic</property>
        <property name="tooltip-text" translatable="true">Remove Port</property>
      </object>
    </child>
  </object>
</interface>
"#;

const STYLES: &str = "
.scope {
    background-color: rgba(0.1, 0.1, 0.1, 1.0);
}
.add-port-form {
    padding: 6px;
    border-spacing: 6px;
}
.port-row {
    padding-left: 6px;
    border-spacing: 6px;
}
";
