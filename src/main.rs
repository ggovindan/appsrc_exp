use gst::prelude::*;
use anyhow::Error;
use derive_more::{Display, Error};
use std::io::{Read};
use std::fs::File;
use std::{thread, time};
use std::os::fd::{AsRawFd, RawFd};
use std::io::prelude::*;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use lazy_static::lazy_static;

use tracing::{info, warn, error, Level, debug};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;

#[cfg(not(target_os = "macos"))]
pub fn run<T, F: FnOnce() -> T + Send + 'static>(main: F) -> T
    where
        T: Send + 'static,
{
    main()
}

#[cfg(target_os = "macos")]
pub fn run<T, F: FnOnce() -> T + Send + 'static>(main: F) -> T
    where
        T: Send + 'static,
{
    use cocoa::appkit::NSApplication;

    use std::thread;

    unsafe {
        let app = cocoa::appkit::NSApp();
        let t = thread::spawn(|| {
            let res = main();

            let app = cocoa::appkit::NSApp();
            app.stop_(cocoa::base::nil);

            // Stopping the event loop requires an actual event
            let event = cocoa::appkit::NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2_(
                cocoa::base::nil,
                cocoa::appkit::NSEventType::NSApplicationDefined,
                cocoa::foundation::NSPoint { x: 0.0, y: 0.0 },
                cocoa::appkit::NSEventModifierFlags::empty(),
                0.0,
                0,
                cocoa::base::nil,
                cocoa::appkit::NSEventSubtype::NSApplicationActivatedEventType,
                0,
                0,
            );
            app.postEvent_atStart_(event, cocoa::base::YES);

            res
        });

        app.run();

        t.join().unwrap()
    }
}

lazy_static! {
    static ref SHARED_FILE: Arc<Mutex<Option<File>>> = Arc::new(Mutex::new(None));
}

#[derive(Debug, Display, Error)]
#[display(fmt = "Received error from {}: {} (debug: {:?})", src, error, debug)]
struct ErrorMessage {
    src: String,
    error: String,
    debug: Option<String>,
    source: glib::Error,
}

fn main_loop(pipeline: gst::Pipeline) -> Result<(), Error> {
    pipeline.set_state(gst::State::Playing);

    let bus = pipeline
        .bus()
        .expect("could not find bus");

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                println!("got eos!!");
                break
            }
            MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null)?;
                return Err(ErrorMessage {
                    src: msg
                        .src()
                        .map(|s| String::from(s.path_string()))
                        .unwrap_or_else(|| String::from("None")),
                    error: err.error().to_string(),
                    debug: Some(err.debug().unwrap().to_string()),
                    source: err.error(),
                }
                    .into());
            }
            _ => (),
        }
    }
    pipeline.set_state(gst::State::Null)?;
    Ok(())
}

const WIDTH: usize = 320;
const HEIGHT: usize = 240;

#[derive(Debug)]
pub struct CustomEOSEvent {
    pub send_eos: bool,
}

impl CustomEOSEvent {
    const EVENT_NAME: &'static str = "custom-eos-event";

    #[allow(clippy::new_ret_no_self)]
    pub fn new(send_eos: bool) -> gst::Event {
        let s = gst::Structure::builder(Self::EVENT_NAME)
            .field("send_eos", send_eos)
            .build();
        gst::event::CustomDownstream::new(s)
    }

    pub fn parse(ev: &gst::EventRef) -> Option<CustomEOSEvent> {
        match ev.view() {
            gst::EventView::CustomDownstream(e) => {
                let s = match e.structure() {
                    Some(s) if s.name() == Self::EVENT_NAME => s,
                    _ => return None, // No structure in this event, or the name didn't match
                };

                let send_eos = s.get::<bool>("send_eos").unwrap();
                Some(CustomEOSEvent { send_eos })
            }
            _ => None, // Not a custom event
        }
    }
}



fn create_pipeline() -> Result<gst::Pipeline, Error> {
    gst::init()?;

    // let pipeline = gst::parse_launch(format!("appsrc name=appsrc do-timestamp=true ! video/mpegts,systemstream=true ! tsdemux name=demux \
    // demux. ! h264parse ! queue ! avdec_h264  ! videoconvert ! avenc_h264_videotoolbox ! mpegtsmux name=mux ! appsink name=appsink").as_str())
    let pipeline = gst::parse_launch(format!("appsrc name=appsrc do-timestamp=true ! video/mpegts,systemstream=true ! tsdemux name=demux \
    demux. ! h264parse ! queue ! avdec_h264  ! videoconvert ! avenc_h264_videotoolbox ! mpegtsmux name=mux ! fdsink name=filesink demux. ! queue ! aacparse ! mux.").as_str())
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();

    let pipeline_weak = pipeline.downgrade();

    let appsrc = pipeline.by_name("appsrc")
        .unwrap()
        .downcast::<gst_app::AppSrc>()
        .unwrap();

    let mut outputfile = File::create("output.ts").expect("unable tot create file");

    let mut i = 1;
    appsrc.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |appsrc, _| {
                info!("getting the file");
                i+=1;

                let mut buffer_new = gst::Buffer::new();
                {
                    let buffer = buffer_new.get_mut().unwrap();
                    buffer.set_size(0);  // Set size to 0 for an empty buffer
                }

                // Push the buffer into the pipeline
                appsrc.push_buffer(buffer_new).unwrap();


                if let Some(pipeline) = pipeline_weak.upgrade() {
                    let ev = CustomEOSEvent::new(true);
                    if !pipeline.send_event(ev) {
                        println!("warning! failed to send eos event")
                    } else {
                        println!("appsrc sending event on pipeline!")
                    }
                }


                //let mut file = File::create(format!("video{}.ts", i)).unwrap();
                // if let Some(pipeline) = pipeline_weak.upgrade() {
                //     let mut file_lock = SHARED_FILE.lock().unwrap();
                //     *file_lock = Some(File::create(format!("{}.ts", i)).unwrap());
                //     if let Some(ref file) = *file_lock {
                //         // Obtain the raw file descriptor
                //         let raw_fd: RawFd = file.as_raw_fd();
                //         info!("Raw file descriptor: {}", raw_fd);
                //         let elem = pipeline.by_name("filesink").unwrap();
                //         elem.set_property("fd", raw_fd);
                //
                //     } else {
                //         // Handle the case where the file is not yet created or is None
                //         info!("No file available.");
                //     }
                // }

                let mut file = File::open("./sample/test_1620_h264.ts").unwrap();
                let mut buffer = Vec::new();
                match file.read_to_end(&mut buffer) {
                    Ok(_) => { info!(cam_id = "snx-c2ahme", "finished reading bytes from file len={}", buffer.len());
                    }
                    Err(e) => {
                        println!("error reading file: {}", e);
                    }
                };
                //let reader = BufReader::new(file);
                //let file_buf = reader.buffer();
                let gst_buffer = gst::Buffer::from_slice(buffer);
                println!("buffer size of teh generated gst_buffer={}", gst_buffer.size());
                let _ = appsrc.push_buffer(gst_buffer);

                println!("--------sleeping for 1 sec-----------");
                thread::sleep(Duration::from_secs(1));

                //appsrc.end_of_stream();

            }).build(),
    );

    let fdsink = pipeline.by_name("filesink").unwrap();
        // .downcast::<gst_app::AppSink>()
        // .unwrap();

    let pipeline_weak = pipeline.downgrade();

    // first set a pad probe
    let sinkpad = fdsink.static_pad("sink").unwrap();

    let mut file_lock = SHARED_FILE.lock().unwrap();
    *file_lock = Some(File::create(format!("{}.ts", 1)).unwrap());
    if let Some(ref file) = *file_lock {
        // Obtain the raw file descriptor
        let raw_fd: RawFd = file.as_raw_fd();
        println!("Raw file descriptor: {}", raw_fd);
        fdsink.set_property("fd", raw_fd);
    } else {
        // Handle the case where the file is not yet created or is None
        error!("No file available.");
    }


    let event_counter = AtomicUsize::new(0);


    sinkpad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |x, probe_info| {
        match probe_info.data {
            Some(gst::PadProbeData::Event(ref ev))
            if ev.type_() == gst::EventType::CustomDownstream => {
                if let Some(custom_event) = CustomEOSEvent::parse(ev) {
                    if let Some(pipeline) = pipeline_weak.upgrade() {
                        println!("closing file!! received custom event on appsink i={}", event_counter.load(Ordering::SeqCst));
                        event_counter.fetch_add(1, Ordering::SeqCst);
                        // we have received the event.. change the file descriptor
                        let mut file_lock = SHARED_FILE.lock().unwrap();
                        *file_lock = Some(File::create(format!("{}.ts", event_counter.load(Ordering::SeqCst))).unwrap());
                        if let Some(ref file) = *file_lock {
                            // Obtain the raw file descriptor
                            let raw_fd: RawFd = file.as_raw_fd();
                            println!("Raw file descriptor: {}", raw_fd);
                            let elem = pipeline.by_name("filesink").unwrap();
                            elem.set_property("fd", raw_fd);
                        } else {
                            // Handle the case where the file is not yet created or is None
                            error!("No file available.");
                        }
                    }
                }
            }
            _ => (),
        }
        gst::PadProbeReturn::Ok
    });


    // appsink.set_callbacks(
    //     gst_app::AppSinkCallbacks::builder()
    //         .new_sample(move |appsink| {
    //             let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    //             let buffer = sample.buffer().ok_or_else(|| {
    //                     println!("Failed to get buffer from appsink");
    //                 gst::FlowError::Error
    //             })?;
    //
    //             let map = buffer.map_readable().map_err(|_| {
    //                     println!("Failed to map buffer readable");
    //                 gst::FlowError::Error
    //             })?;
    //
    //             outputfile.write_all(map.as_slice()).expect("unable to write the file");
    //
    //             Ok(gst::FlowSuccess::Ok)
    //         })
    //         .build()
    // );

    Ok(pipeline)

}

fn appsrc_main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer()
            // Enable JSON formatter
            .json()
            // Include file name, line number, target, etc.
            .with_span_list(false)
            .with_span_events(FmtSpan::CLOSE)
            .with_file(true)
            .with_line_number(true)
            .with_target(true)
        )
        .init();
    match create_pipeline().and_then(main_loop) {
        Ok(r) => r,
        Err(e) => println!("Error! {}", e),
    }
}

fn main() {
    run(appsrc_main);
}
