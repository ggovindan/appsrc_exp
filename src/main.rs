use gst::prelude::*;
use anyhow::Error;
use derive_more::{Display, Error};
use gst::BufferFlags;
use std::io::{BufReader, Read};
use std::fs::File;
use std::{thread, time};

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
                    debug: Some("".to_string()),
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

// gst-launch-1.0 videotestsrc ! videoconvert ! avenc_h264_videotoolbox ! mpegtsmux ! filesink location="dip.ts"
fn create_pipeline() -> Result<gst::Pipeline, Error> {
    gst::init()?;

    let pipeline = gst::Pipeline::default();

    let video_info = &gst::Caps::builder("video/mpegts")
        .field("systemstream", true)
        .build();

    let appsrc = gst_app::AppSrc::builder()
        .caps(video_info)
        .format(gst::Format::Time)
        .is_live(false)
        .build();

    // gst-launch-1.0 filesrc location=dip.ts ! tsdemux name=demux demux. ! h264parse ! decodebin ! videoconvert ! autovideosink
    // gst-launch-1.0 filesrc location=dip.ts ! tsdemux name=demux demux. ! h264parse ! mpegtsmux ! filesink location=pp.ts
    let tsdemux = gst::ElementFactory::make("tsdemux").build()?;
    let h264parse = gst::ElementFactory::make("h264parse").build()?;
    let mpegtsmux = gst::ElementFactory::make("mpegtsmux").build()?;
    let filesink = gst::ElementFactory::make("multifilesink").build()?;
    //let filesink = gst::ElementFactory::make("filesink").build()?;
    filesink.set_property("location", "%d.ts");
    //filesink.set_property("post-messages", true);
    filesink.set_property_from_str("next-file", "buffer");

    // moved out of lambda to avoid move closures
    let video_sink_pad = h264parse.static_pad("sink").expect("could not get sink pad from h264parse");

    pipeline.add_many(&[appsrc.upcast_ref(), &tsdemux, h264parse.upcast_ref(), &mpegtsmux, &filesink])?;
    gst::Element::link_many(&[appsrc.upcast_ref(), &tsdemux])?;

    tsdemux.connect_pad_added(move |_src, src_pad| {
        let is_video = if src_pad.name().starts_with("video") {
            true
        } else {
            false
        };

        let connect_demux = || -> Result<(), Error> {
            src_pad.link(&video_sink_pad).expect("failed to link tsdemux.video->h264parse.sink");
            println!("linked tsdemux->h264parse");
            Ok(())
        };

        if is_video {
            match connect_demux() {
                Ok(_) => println!("tsdemux->h264 connected"),
                Err(e) => println!("could not connect tsdemux->h264parse e:{}", e),
            }
        }
    });


    gst::Element::link_many(&[&h264parse.upcast_ref(), &mpegtsmux, &filesink])?;

     appsrc.set_callbacks(
         gst_app::AppSrcCallbacks::builder()
             .need_data(move |appsrc, _| {
                 println!("getting the file");
                 let mut file = File::open("./sample/test_1620.ts").unwrap();
                 let mut buffer = Vec::new();
                 match file.read_to_end(&mut buffer) {
                     Ok(_) => {
                         println!("finished reading bytes from file len={}", buffer.len());
                     }
                     Err(e) => {
                         println!("error reading file: {}", e);
                     }
                 };
                 //let reader = BufReader::new(file);
                 //let file_buf = reader.buffer();
                 let gst_buffer = gst::Buffer::from_slice(buffer);
                 println!("buffer size of teh generated gst_buffer={}", gst_buffer.size());
                 appsrc.end_of_stream();
                 let _ = appsrc.push_buffer(gst_buffer);
                 println!("sleeping for 10 secs");
                 thread::sleep(time::Duration::from_secs(10));
             }).build(),
     );

    Ok(pipeline)

}

fn appsrc_main() {
    match create_pipeline().and_then(main_loop) {
        Ok(r) => r,
        Err(e) => println!("Error! {}", e),
    }
}

fn main() {
    run(appsrc_main);
}
