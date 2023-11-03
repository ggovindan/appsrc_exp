use gst::prelude::*;
use anyhow::Error;
use derive_more::{Display, Error};
use gst::BufferFlags;

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
            MessageView::Eos(..) => break,
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

    let video_info = gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgrx, WIDTH as u32, HEIGHT as u32)
        .fps(gst::Fraction::new(2, 1))
        .build()
        .expect("Failed to create video info");

    let appsrc = gst_app::AppSrc::builder()
        .caps(&video_info.to_caps().unwrap())
        .format(gst::Format::Time)
        .build();

    let videoconvert = gst::ElementFactory::make("videoconvert").build()?;
    let avenc = gst::ElementFactory::make("avenc_h264_videotoolbox").build()?;
    let mpegtsmux = gst::ElementFactory::make("mpegtsmux").build()?;
    let filesink = gst::ElementFactory::make("multifilesink").build()?;
    filesink.set_property("location", "%d.ts");
    filesink.set_property("post-messages", true);
    filesink.set_property_from_str("next-file", "discont");

    pipeline.add_many(&[appsrc.upcast_ref(), &videoconvert, &avenc, &mpegtsmux, &filesink])?;
    gst::Element::link_many(&[appsrc.upcast_ref(), &videoconvert, &avenc, &mpegtsmux, &filesink])?;

    let mut i = 0;

    appsrc.set_callbacks(
        gst_app::AppSrcCallbacks::builder()
            .need_data(move |appsrc, _| {
                if i == 100 {
                    let _ = appsrc.end_of_stream();
                    println!("sending EOS to appsrc");
                    return;
                }

                println!("producing frame {}", i);

                let r = if i % 2 == 0 { 0 } else { 255 };
                let g = if i % 3 == 0 { 0 } else { 255 };
                let b = if i % 5 == 0 { 0 } else { 255 };

                let mut buffer = gst::Buffer::with_size(video_info.size()).unwrap();

                {
                    let buffer = buffer.get_mut().unwrap();
                    if i % 10 == 0 {
                        buffer.set_flags(BufferFlags::LIVE | BufferFlags::DISCONT)
                    }

                    // let the player know when to play this frame
                    buffer.set_pts(i * 500 * gst::ClockTime::MSECOND);

                    let mut vframe = gst_video::VideoFrameRef::from_buffer_ref_writable(buffer, &video_info).unwrap();

                    let width = vframe.width() as usize;
                    let height = vframe.height() as usize;

                    let stride = vframe.plane_stride()[0] as usize;

                    for line in vframe
                        .plane_data_mut(0)
                        .unwrap()
                        .chunks_exact_mut(stride)
                        .take(height)
                    {
                        // Iterate over each pixel of 4 bytes in that line
                        for pixel in line[..(4 * width)].chunks_exact_mut(4) {
                            pixel[0] = b;
                            pixel[1] = g;
                            pixel[2] = r;
                            pixel[3] = 0;
                        }
                    }
                }
                i += 1;

                let _ = appsrc.push_buffer(buffer);
            })
            .build(),
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
