//! Bounded WebContent-stall regression using the production native overlay.
//! Run with TIPTOPTYP_NATIVE_ZOOM_TEST=1, optionally --web-thread for baseline.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/native_window.rs"]
mod native_window;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/preview_zoom_snapshot.rs"]
mod preview_zoom_snapshot;

fn main() {
    if std::env::var("TIPTOPTYP_NATIVE_ZOOM_TEST").as_deref() != Ok("1") {
        println!(
            "native preview zoom skipped; set TIPTOPTYP_NATIVE_ZOOM_TEST=1 in a desktop session"
        );
        return;
    }
    #[cfg(target_os = "macos")]
    run();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("native preview zoom requires macOS with WebKit and a desktop session");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run() {
    use objc2::{MainThreadMarker, MainThreadOnly, class, msg_send, runtime::AnyObject};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent,
        NSEventModifierFlags, NSEventType, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    use wry::WebViewExtMacOS;
    let baseline = std::env::args().any(|arg| arg == "--web-thread");
    let live_url = std::env::var("TIPTOPTYP_NATIVE_ZOOM_URL").ok();
    let marker = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(marker);
    let background = std::env::args().any(|arg| arg == "--background");
    if background {
        // Wry's child constructor queues NSApp.activate. Keep that prohibited
        // for this entire nonactivating owned-window fixture.
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        assert_eq!(
            app.activationPolicy(),
            NSApplicationActivationPolicy::Prohibited
        );
    }
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(marker),
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(640.0, 420.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(&NSString::from_str("Native zoom isolation regression"));
    let parent = native_window::NativeWindowHandle::from_test_view(window.contentView().unwrap());
    let (sender, receiver) = mpsc::channel();
    let enabled = !baseline;
    let pixel_anchors = std::env::args().any(|arg| arg == "--pixel-anchors");
    let helper = include_str!("../src/preview_zoom_snapshot.js");
    let buffer = include_str!("../src/preview_snapshot_buffer.js");
    let html = format!(
        r##"<html><body style="margin:0;background:#eee;overflow:hidden"><svg id="drawing" width="640" height="1260"><rect width="640" height="420" fill="#fff"/><path d="M30 25 L30 120 L200 120 M50 45 L150 45" stroke="#163b50" stroke-width="5" fill="none"/><text x="30" y="175" font-size="32">Cached native zoom</text><rect x="550" y="20" width="50" height="40" fill="#d55427"/><rect x="300" y="100" width="20" height="10" fill="#ff0000"/></svg><script>
    window.tiptoptypCaptureZoomSnapshot=m=>window.ipc.postMessage(JSON.stringify({{type:'preview-zoom-snapshot',...m}}));
    {helper}
    {buffer}
    const frame={{doc:{{}},svg:document.getElementById('drawing'),scale:1,width:640,height:420,fitWidth:640,left:0,top:0,background:[238,238,238],geometry:{{paper:[0,0,640,1260],viewport:[0,0,640,420],view:[640,420],scroll:[0,0]}},ready:true}};
    if ({pixel_anchors}) {{
      document.body.style.background = '#525659';
      frame.svg.innerHTML = [0,1,2].map(index=>`<rect class="typst-page-inner" data-page-number="${{index}}" x="0" y="${{index*210}}" width="640" height="200" fill="white"/><g class="typst-page" data-page-number="${{index}}"><rect x="300" y="${{index*210+100}}" width="20" height="10" fill="${{['#ff0000','#00ff00','#0000ff'][index]}}"/></g>`).join('');
      frame.geometry = {{paper:[0,0,640,620],viewport:[0,0,640,420],view:[640,420],scroll:[0,0],source_scale:1,pages:[[0,200],[210,200],[420,200]]}};
    }}
    const send=m=>window.ipc.postMessage(JSON.stringify(m));
    window.tiptoptypZoomSnapshot.attach(()=>frame);
    window.tiptoptypZoomSnapshot.configure({enabled});
    window.addEventListener('resize',()=>window.tiptoptypZoomSnapshot.resize());
    let requests=0;
    window.tiptoptypNativeZoom=request=>{{
      requests++; window.tiptoptypZoomSnapshot.native(request); frame.ready=false;
      setTimeout(()=>{{frame.scale=request.scale;frame.width=innerWidth;frame.height=innerHeight;frame.fitWidth=innerWidth;frame.ready=true;frame.svg.style.transformOrigin='0 0';frame.svg.style.transform=`scale(${{frame.scale*innerWidth/640}})`;frame.svg.style.position='absolute';frame.svg.style.left=`${{Math.max(0,(innerWidth-Math.ceil(innerWidth*frame.scale))/2)}}px`;const rect=frame.svg.getBoundingClientRect();frame.geometry={{paper:[rect.left,rect.top,rect.width,rect.height],viewport:[0,0,innerWidth,innerHeight],view:[innerWidth,innerHeight],scroll:[0,0]}};window.tiptoptypZoomSnapshot.rendered();send({{type:'vector',scale:frame.scale,requests}});}},30);return true;
    }};
    window.addEventListener('keydown',event=>{{
      if((event.metaKey||event.ctrlKey)&&event.key==='='){{event.preventDefault();frame.scale*=1.1;frame.svg.style.transformOrigin='0 0';frame.svg.style.transform=`scale(${{frame.scale}})`;send({{type:'web-input',scale:frame.scale,at:Date.now()}});}}
    }});
    window.blockRenderer=()=>{{send({{type:'blocked',at:Date.now()}});const until=performance.now()+800;while(performance.now()<until){{}}send({{type:'unblocked',at:Date.now()}});}};
    send({{type:'loaded'}});
    </script></body></html>"##
    );
    let mut builder = wry::WebViewBuilder::new();
    if let Some(url) = &live_url {
        let adapter = include_str!("../src/preview_navigation.js");
        let dark = std::env::args().any(|arg| arg == "--dark");
        let setup = format!(
            r#"
            let requests=0;
            const send=m=>window.ipc.postMessage(JSON.stringify(m));
            window.addEventListener('error',event=>send({{type:'frontend-error',kind:'error',message:String(event.message).slice(0,240),line:event.lineno,column:event.colno}}));
            window.addEventListener('unhandledrejection',event=>send({{type:'frontend-error',kind:'rejection',message:String(event.reason?.message??event.reason).slice(0,240)}}));
            window.tiptoptypCaptureZoomSnapshot=m=>{{
                let descriptor=null;
                if(m.op==='capture'){{
                    try{{const frame=window.tiptoptypNativeZoomFrame?.();
                        descriptor={{scale:frame?.scale,location:window.tiptoptypNativeZoomPageLocation?.(frame)}};
                    }}catch(error){{descriptor={{error:String(error?.message??error).slice(0,240)}};}}
                }}
                send({{type:'preview-zoom-snapshot',...m,test_capture_descriptor:descriptor}});
                if(m.op==='ready'){{send({{type:'vector',requests,scale:m.scale}});window.tiptoptypNativeZoomObserve?.('handoff');}};
            }};
            {helper}
            {buffer}
            const prepareBuffer=window.tiptoptypSnapshotBuffer;
            window.tiptoptypSnapshotBuffer=frame=>{{
                const started=performance.now();
                try{{return prepareBuffer(frame);}}
                finally{{send({{type:'buffer-preparation',ms:performance.now()-started}});}}
            }};
            const attach=window.tiptoptypZoomSnapshot.attach;
            window.tiptoptypZoomSnapshot.attach=reader=>{{window.tiptoptypNativeZoomFrame=reader;return attach(reader);}};
            window.tiptoptypNativeZoomPageLocation=frame=>{{
                const pages=[...(frame?.svg?.children??[])].filter(node=>node.matches('g.typst-page'));
                const page=pages[18],matrix=page?.getScreenCTM();
                const source_height=Number(page?.getAttribute('data-page-height'));
                const height=source_height*(matrix?.d??0);
                const next=pages[19]?.getScreenCTM();
                return matrix&&height>0?{{page:18,top:matrix.f,height,source_height,source_scale:matrix.d,
                    gap:next?next.f-matrix.f-height:null,
                    ink:page.getAttribute('data-dummy')!=='1'&&!!page.querySelector('use,path,text'),
                    fraction:(frame.geometry.viewport[1]-matrix.f)/height}}:null;
            }};
            window.tiptoptypNativeZoomObserve=(label,request_id=null)=>{{
                let frame,location=null,cache=null,error=null;
                try{{
                    frame=window.tiptoptypNativeZoomFrame?.();
                    location=window.tiptoptypNativeZoomPageLocation(frame);
                    cache=window.tiptoptypZoomSnapshot.inspect();
                }}catch(failure){{error={{name:String(failure?.name??'Error'),message:String(failure?.message??failure).slice(0,240)}};}}
                send({{type:'zoom-observation',label,request_id,at:performance.now(),ready:!!frame?.ready&&!error,
                    available:!!frame&&!error,error,visibility:document.visibilityState,
                    scale:frame?.scale,geometry:frame?.geometry,location,
                    scroll_limits:frame?.doc?.hookedElem?.parentElement ? [frame.doc.hookedElem.parentElement.scrollWidth-frame.doc.hookedElem.parentElement.clientWidth,frame.doc.hookedElem.parentElement.scrollHeight-frame.doc.hookedElem.parentElement.clientHeight] : null,
                    cache,request:window.tiptoptypLastNativeZoom}});
            }};
            {adapter}
            const zoom=window.tiptoptypNativeZoom;
            window.tiptoptypNativeZoom=req=>{{requests++;window.tiptoptypLastNativeZoom=req;return zoom(req);}};
            window.tiptoptypZoomSnapshot.configure(true);
            window.blockRenderer=()=>{{send({{type:'blocked'}});const until=performance.now()+800;while(performance.now()<until){{}}send({{type:'unblocked'}});}};
            let lastDiagnostic=-Infinity;
            const diagnostic=phase=>{{
                const doc=document.getElementById('typst-container')?.documents?.[0]?.impl;
                let frame=null,readerError=null;
                try{{frame=window.tiptoptypNativeZoomFrame?.();}}
                catch(error){{readerError={{name:String(error?.name??'Error'),message:String(error?.message??error).slice(0,240)}};}}
                send({{type:'frontend-diagnostic',phase,documentState:document.readyState,visibility:document.visibilityState,readerError,
                    rendererPresent:!!doc,moduleInitialized:!!doc?.moduleInitialized,isRendering:!!doc?.isRendering,
                    queued:doc?.patchQueue?.length??null,frameReady:!!frame?.ready,
                    cache:window.tiptoptypZoomSnapshot.inspect()}});
            }};
            const ready=()=>{{const doc=document.getElementById('typst-container')?.documents?.[0]?.impl;
                if(doc?.moduleInitialized){{
                    if({dark})window.tiptoptypSetPalette([40,40,40],[235,219,178],'1px solid transparent','1px solid transparent',[250,189,47]);
                    send({{type:'loaded',partial:doc.partialRendering}});
                }}
                else{{if(performance.now()-lastDiagnostic>500){{lastDiagnostic=performance.now();diagnostic('waiting');}}setTimeout(ready,25);}}
            }};
            diagnostic('initialization-installed');
            window.addEventListener('DOMContentLoaded',ready);
        "#
        );
        builder = builder.with_url(url).with_initialization_script(setup);
    } else {
        builder = builder.with_html(html);
    }
    let web = builder
        .with_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
            size: wry::dpi::LogicalSize::new(640.0, 420.0).into(),
        })
        .with_hotkeys_zoom(false)
        .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
        .with_ipc_handler(move |request| {
            let _ = sender.send(request.body().clone());
        })
        .build_as_child(&parent)
        .unwrap();
    let state = preview_zoom_snapshot::CaptureState::default();
    state.bind(&web);
    state.allowed(enabled);
    if background {
        // macOS 14 throttles hidden/occluded WebKit animation frames despite
        // its background policy. Give this owned view a paintable window,
        // without making it key or activating the test application.
        // Floating level keeps this paintable fixture above ordinary editor
        // windows without taking application/keyboard focus.
        window.setLevel(3);
        window.orderFront(None);
        assert!(window.isVisible(), "owned floating fixture is not visible");
    } else {
        window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
    web.focus().unwrap();
    let frontmost_pid = || unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let application: Option<objc2::rc::Retained<AnyObject>> =
            msg_send![&*workspace, frontmostApplication];
        application.map(|application| {
            let pid: i32 = msg_send![&*application, processIdentifier];
            pid
        })
    };
    if background {
        assert_ne!(
            frontmost_pid(),
            Some(std::process::id() as i32),
            "owned visible fixture activated the test application"
        );
    }
    let start = Instant::now();
    let mut loaded = false;
    let mut actual_partial = None;
    let mut loaded_at = None;
    let mut block_started = None;
    let mut unblocked = false;
    let mut posted = 0;
    let mut input_times = Vec::new();
    let mut response_times = Vec::new();
    let mut observed = Vec::new();
    let mut web_inputs = Vec::new();
    let mut requests = 0;
    let mut final_scale = 1.0;
    let pump = || unsafe {
        let date: *mut AnyObject = msg_send![class!(NSDate),dateWithTimeIntervalSinceNow:0.005f64];
        let event: Option<objc2::rc::Retained<NSEvent>> = msg_send![&*app,nextEventMatchingMask:usize::MAX,untilDate:date,inMode:&*NSString::from_str("kCFRunLoopDefaultMode"),dequeue:true];
        if let Some(event) = event {
            app.sendEvent(&event);
        }
    };
    let output = std::env::var_os("TIPTOPTYP_NATIVE_ZOOM_RECEIPT").map(std::path::PathBuf::from);
    let mut capture_timings = CaptureTimings::default();
    while start.elapsed() < Duration::from_secs(30) {
        pump();
        while let Ok(message) = receiver.try_recv() {
            let value: serde_json::Value = serde_json::from_str(&message).unwrap();
            capture_timings.record(&value);
            match value["type"].as_str() {
                Some("preview-zoom-snapshot") => state.message(&web, &value, enabled),
                Some("loaded") => {
                    actual_partial = value["partial"].as_bool();
                    loaded = true;
                    loaded_at = Some(Instant::now());
                }
                Some("blocked") => block_started = Some(Instant::now()),
                Some("unblocked") => unblocked = true,
                Some("web-input") => {
                    if let Some(block) = block_started {
                        response_times.push(block.elapsed().as_secs_f64() * 1000.0);
                    }
                    web_inputs.push(value);
                }
                Some("vector") => {
                    requests = value["requests"].as_u64().unwrap();
                    final_scale = value["scale"].as_f64().unwrap();
                }
                _ => {}
            }
        }
        if loaded
            && loaded_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(3))
            && block_started.is_none()
            && (baseline || state.inspect()["cached"] == true)
        {
            if !baseline {
                assert!(
                    window.firstResponder().is_some_and(|responder| unsafe {
                        msg_send![&*responder, isDescendantOf: &*web.webview()]
                    }),
                    "idle buffer took keyboard focus"
                );
                assert_eq!(
                    state.inspect()["capture_view"],
                    serde_json::json!([1.0, 1.0]),
                    "idle capture view retained its large document surface"
                );
                assert_eq!(
                    state.inspect()["image_origin"],
                    serde_json::json!([-320.0, -420.0])
                );
                assert_eq!(
                    state.inspect()["image_size"],
                    serde_json::json!([1280.0, 1260.0])
                );
            }
            if std::env::args().any(|arg| arg == "--pixel-anchors") {
                assert!(
                    live_url.is_none(),
                    "pixel marker uses the owned SVG fixture"
                );
                // Run directly from the first warm, idle capture: page slices have
                // unique colors and source positions, independent of native transforms.
                assert_eq!(state.inspect()["image_layers"], 3);
                let mut receipt = serde_json::json!({"viewport":[640,420],"source_pages":[[0,200],[210,200],[420,200]],"warmup_seconds":3});
                let file = std::env::var_os("TIPTOPTYP_NATIVE_ZOOM_PNG")
                    .expect("pixel anchors require a PNG evidence path");
                let file = std::path::PathBuf::from(file);
                let capture = |label: &str| {
                    let path = file.with_extension(format!("{label}.png"));
                    state.write_overlay_png(&path);
                    let pixels = image::open(&path).unwrap().to_rgba8();
                    let mut sums = [[0.; 3]; 3];
                    for (px, py, pixel) in pixels.enumerate_pixels() {
                        for (index, sums) in sums.iter_mut().enumerate() {
                            if pixel[index] > 220
                                && (0..3)
                                    .filter(|channel| *channel != index)
                                    .all(|channel| pixel[channel] < 20)
                            {
                                sums[0] += f64::from(px);
                                sums[1] += f64::from(py);
                                sums[2] += 1.;
                            }
                        }
                    }
                    assert!(
                        sums[0][2] > 0. && sums[1][2] > 0.,
                        "missing first/second page markers: {}",
                        path.display()
                    );
                    assert_eq!(
                        sums[2][2], 0.,
                        "third page incorrectly appeared in the first two page slices"
                    );
                    [
                        [sums[0][0] / sums[0][2], sums[0][1] / sums[0][2]],
                        [sums[1][0] / sums[1][2], sums[1][1] / sums[1][2]],
                    ]
                };
                // Reveal the idle raster without materially changing scale.
                state.message(
                    &web,
                    &serde_json::json!({"op":"zoom","factor":1.00000000001,"x":0,"y":0}),
                    true,
                );
                let before = capture("before");
                web.set_bounds(wry::Rect {
                    position: wry::dpi::LogicalPosition::new(0., 0.).into(),
                    size: wry::dpi::LogicalSize::new(560., 420.).into(),
                })
                .unwrap();
                state.layout(&web);
                let resized = capture("resize");
                state.message(
                    &web,
                    &serde_json::json!({"op":"zoom","factor":1.2,"x":100,"y":300}),
                    true,
                );
                let pointer = capture("pointer");
                // Ten-pixel gaps do not scale. At y=300 the cursor is on page two:
                // preserve its source point, then independently project both markers.
                let expectations = [
                    [[310., 105.], [310., 315.]],
                    [[271.25, 91.875], [271.25, 276.875]],
                    [[305.5, 52.25], [305.5, 272.25]],
                ];
                let observed = [before, resized, pointer];
                let errors = observed
                    .iter()
                    .zip(expectations)
                    .map(|(a, b)| {
                        std::array::from_fn::<_, 2, _>(|page| {
                            [a[page][0] - b[page][0], a[page][1] - b[page][1]]
                        })
                    })
                    .collect::<Vec<_>>();
                let passed = errors
                    .iter()
                    .flatten()
                    .flatten()
                    .all(|value| value.abs() <= 1.0);
                receipt["pixel_anchors"] = serde_json::json!({"passed":passed,"source_marker_centers":[[310,105],[310,315],[310,525]],"inputs":{"resize":[640,560],"zoom":1.2,"cursor":[100,300]},"observed":observed,"expected":expectations,"errors_css_pixels":errors,"scope":"offscreen CALayer page pixels in host coordinates; composed GPU presentation checked separately"});
                // Fit all three papers inside the viewport. Pixels beyond the
                // last paper must match the live gutter, not frame.background.
                state.message(
                    &web,
                    &serde_json::json!({"op":"zoom","factor":0.35 / 1.2,"x":100,"y":0}),
                    true,
                );
                let below_path = file.with_extension("below-document.png");
                state.write_overlay_png(&below_path);
                let below = image::open(&below_path).unwrap().to_rgba8();
                let backdrop = below.get_pixel(280, 410).0;
                receipt["below_document_backdrop"] = serde_json::json!({"observed":backdrop,"expected":[82,86,89,255],"passed":backdrop==[82,86,89,255]});
                if let Some(path) = output.as_deref() {
                    std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
                }
                state.allowed(false);
                window.orderOut(None);
                assert!(
                    passed,
                    "cached pixels did not preserve the independent top/cursor anchor: {errors:?}"
                );
                assert_eq!(
                    backdrop,
                    [82, 86, 89, 255],
                    "viewport below the final paper differs from the live backdrop"
                );
                println!("{receipt}");
                return;
            }
            if std::env::args().any(|arg| arg == "--image-bench") {
                // Measure only production AppKit submission, excluding startup,
                // capture, vector rendering and disk writes. Owned nonactivating window.
                let zoom_in = serde_json::json!({"op":"zoom","factor":1.01,"x":635,"y":415});
                let zoom_out = serde_json::json!({"op":"zoom","factor":1.0/1.01,"x":635,"y":415});
                for _ in 0..64 {
                    state.message(&web, &zoom_in, true);
                    state.message(&web, &zoom_out, true);
                }
                let mut batches = Vec::new();
                for _ in 0..5 {
                    let at = Instant::now();
                    for _ in 0..1000 {
                        state.message(&web, &zoom_in, true);
                        state.message(&web, &zoom_out, true);
                    }
                    batches.push(at.elapsed().as_secs_f64() * 1000.0);
                }
                let result = serde_json::json!({"workload":"five batches of 2000 alternating 1% cached zooms at bottom-right; 128 warmup events", "profile":"release", "viewport":[640,420],"background":background,"events_per_batch":2000,"submission_ms":batches,"native":state.inspect()});
                if let Some(path) = output.as_deref() {
                    std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
                }
                println!("{result}");
                state.allowed(false);
                window.orderOut(None);
                return;
            }
            web.evaluate_script("window.blockRenderer();").unwrap();
            loaded = false;
        }
        if let Some(block) = block_started {
            let elapsed = block.elapsed().as_secs_f64() * 1000.0;
            if posted < 4 && elapsed >= 100.0 + posted as f64 * 60.0 {
                let event=NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                    NSEventType::KeyDown,NSPoint::new(100.0,100.0),NSEventModifierFlags::Command,0.0,window.windowNumber(),None,
                    &NSString::from_str("="),&NSString::from_str("="),false,24).unwrap();
                app.postEvent_atStart(&event, false);
                input_times.push(elapsed);
                posted += 1;
            }
            if !baseline {
                let hits = state.inspect()["hits"].as_u64().unwrap() as usize;
                while response_times.len() < hits {
                    response_times.push(elapsed);
                }
            }
            if !unblocked && elapsed > 350.0 && observed.is_empty() {
                observed.push(serde_json::json!({"elapsed_ms":elapsed,"renderer_still_blocked":true,"native":state.inspect(),"web_inputs":web_inputs.len()}));
                if !baseline && let Some(path) = std::env::var_os("TIPTOPTYP_NATIVE_ZOOM_PNG") {
                    state.write_overlay_png(std::path::Path::new(&path));
                }
            }
            if unblocked
                && ((baseline && web_inputs.len() == 4)
                    || (!baseline
                        && requests == 1
                        && (final_scale - 1.1f64.powi(4)).abs() < 1e-9
                        && state.inspect()["active"] == false))
            {
                break;
            }
        }
    }
    let responsive = observed
        .first()
        .is_some_and(|value| value["native"]["hits"] == 4 && value["native"]["active"] == true);
    let mut receipt = serde_json::json!({"mode":if baseline{"web-thread"}else{"native-compositor"},
        "window_mode":if background{"nonactivating visible owned floating window"}else{"foreground owned window"},
        "window_state":{"visible":window.isVisible(),"occlusion":window.occlusionState().bits(),"level":window.level()},"viewport":[640,420],"warmup_seconds":3,"blocked_ms":800,"posted_keys":posted,
        "during_block":observed,"input_times_ms":input_times,"response_times_ms":response_times,
        "input_response_ms":input_times.iter().zip(&response_times).map(|(input,response)|response-input).collect::<Vec<_>>(),"responsive_during_block":responsive,"vector_requests":requests,"final_scale":final_scale,"native":state.inspect(),"web_inputs":web_inputs.len(),
        "actual_tinymist":live_url.is_some(),"partial":actual_partial,"frontend_loaded":loaded_at.is_some(),
        "ready_to_block":loaded,"capture_timings":capture_timings.inspect()});
    if let Some(path) = output.as_deref() {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    assert_eq!(posted, 4);
    assert!(unblocked, "WebContent block never completed");
    if baseline {
        assert!(
            !responsive,
            "Baseline unexpectedly transformed outside WebContent"
        );
        assert_eq!(web_inputs.len(), 4);
    } else {
        assert!(
            responsive,
            "Native bitmap did not respond while WebContent was blocked"
        );
        assert_eq!(
            requests, 1,
            "Cached gesture started vector work before settling"
        );
        let during = observed.first().unwrap();
        assert!(
            during["native"]["in_flight"].is_null(),
            "Vector work began during the gesture"
        );
        assert_eq!(during["native"]["pending"], 4);
        assert_eq!(
            web_inputs.len(),
            0,
            "Native keys leaked into the browser's duplicate zoom path"
        );
    }
    // Back-to-back input must retain image feedback even while a replacement
    // capture or vector work occupies WebContent. This bounded probe is also
    // used with a preserved baseline implementation for matched release timing.
    if std::env::args().any(|arg| arg == "--handoff-bench") {
        let before = state.inspect();
        let handoffs = before["handoffs"].as_u64().unwrap();
        let hits = before["hits"].as_u64().unwrap();
        web.evaluate_script("window.blockRenderer();").unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut blocked = false;
        while Instant::now() < deadline && !blocked {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
                if value["type"] == "blocked" {
                    blocked = true;
                }
            }
        }
        assert!(blocked);
        let at = Instant::now();
        assert!(state.action(&serde_json::json!({"action":"zoom-in"})));
        let feedback = if state.inspect()["hits"].as_u64().unwrap() > hits {
            Some(at.elapsed().as_secs_f64() * 1_000_000.0)
        } else {
            None
        };
        let during = state.inspect();
        while Instant::now() < deadline && state.inspect()["handoffs"].as_u64().unwrap() == handoffs
        {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
            }
        }
        assert!(state.inspect()["handoffs"].as_u64().unwrap() > handoffs);
        receipt["handoff_probe"] = serde_json::json!({"cached_after_sharp":before["cached"],
            "cached_feedback_us":feedback,"input_to_sharp_ms":at.elapsed().as_secs_f64()*1000.0,
            "during_block":during,"after":state.inspect()});
        if let Some(path) = output.as_deref() {
            std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
        }
        println!("{receipt}");
        state.allowed(false);
        window.orderOut(None);
        return;
    }
    if !baseline {
        assert_eq!(
            state.inspect()["cached"],
            true,
            "sharp handoff discarded the valid cache"
        );
    }

    if std::env::args().any(|arg| arg == "--refresh-race") {
        let before = state.inspect();
        web.evaluate_script(
            r#"const original=window.tiptoptypZoomSnapshot.resolve;
            window.tiptoptypZoomSnapshot.resolve=(...args)=>{window.heldSnapshot=args;};
            window.releaseSnapshot=()=>{window.tiptoptypZoomSnapshot.resolve=original;
                original(...window.heldSnapshot);delete window.heldSnapshot;};"#,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && state.inspect()["prepared"] != true {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
            }
        }
        let prepared = state.inspect();
        assert_eq!(
            prepared["prepared"], true,
            "refresh never staged a replacement"
        );
        assert_eq!(
            prepared["source_paper"], before["source_paper"],
            "staging changed the admitted image geometry"
        );
        assert_eq!(prepared["base_scale"], before["base_scale"]);
        assert_eq!(prepared["cached"], true);
        let hits = prepared["hits"].as_u64().unwrap();
        let handoffs = prepared["handoffs"].as_u64().unwrap();
        assert!(state.action(&serde_json::json!({"action":"zoom-in"})));
        let raced = state.inspect();
        assert_eq!(raced["prepared"], false);
        assert_eq!(raced["hits"].as_u64().unwrap(), hits + 1);
        assert_eq!(raced["source_paper"], before["source_paper"]);
        web.evaluate_script("window.releaseSnapshot();").unwrap();
        while Instant::now() < deadline {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
            }
            let current = state.inspect();
            if current["handoffs"].as_u64().unwrap() > handoffs
                && current["cached"] == true
                && current["base_scale"] == current["scale"]
            {
                break;
            }
        }
        let after = state.inspect();
        assert!(after["handoffs"].as_u64().unwrap() > handoffs);
        assert_eq!(after["cached"], true);
        assert_eq!(
            after["base_scale"], after["scale"],
            "raced refresh did not recover with a new admitted image"
        );
        receipt["refresh_race"] =
            serde_json::json!({"before":before,"prepared":prepared,"raced":raced,"after":after});
    }
    // Zoom-out exposes gutters around the buffered paper. They must be opaque,
    // and divider bounds must move the image while the live renderer is blocked.
    // These are owned-window operations; --background never posts global input.
    if !baseline && live_url.is_none() {
        let before = state.inspect();
        state.message(
            &web,
            &serde_json::json!({"op":"zoom","factor":0.1 / before["scale"].as_f64().unwrap(),"x":320,"y":210}),
            enabled,
        );
        let uncovered = state.inspect();
        assert_eq!(
            uncovered["cached"], true,
            "uncaptured paper discarded reusable raster geometry"
        );
        assert_eq!(uncovered["active"], false);
        assert_eq!(uncovered["source_paper"], before["source_paper"]);
        assert_eq!(uncovered["hits"], before["hits"]);
        assert!(
            !uncovered["in_flight"].is_null(),
            "cold fallback was delayed"
        );
        let deadline = Instant::now() + Duration::from_secs(8);
        while state.inspect()["handoffs"].as_u64().unwrap() == before["handoffs"].as_u64().unwrap()
        {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
            }
            assert!(Instant::now() < deadline, "cold zoom never handed off");
        }
        receipt["uncovered_fallback"] = uncovered;
        assert!(state.action(&serde_json::json!({"action":"fit"})));
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
            }
            let current = state.inspect();
            if current["cached"] == true
                && current["active"] == false
                && current["base_scale"] == current["scale"]
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fit did not produce a fresh idle image"
            );
        }
        web.evaluate_script("document.body.style.background='#f0f';window.blockRenderer();")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            pump();
            let blocked = receiver.try_iter().any(|message| {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
                value["type"] == "blocked"
            });
            if blocked {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "second WebContent block did not start"
            );
        }
        state.message(
            &web,
            &serde_json::json!({"op":"zoom","factor":0.8,"x":320,"y":210}),
            enabled,
        );
        let zoom_out = state.inspect();
        assert_eq!(zoom_out["transform"], serde_json::json!([0.8, 64.0, 0.0]));
        if let Some(path) = std::env::var_os("TIPTOPTYP_NATIVE_ZOOM_PNG") {
            let path = std::path::PathBuf::from(path).with_file_name("native-buffer-zoom-out.png");
            state.write_overlay_png(&path);
            let pixels = image::open(&path).unwrap().to_rgba8();
            assert_eq!(pixels.dimensions(), (640, 420));
            assert_eq!(
                pixels.get_pixel(10, 200).0,
                [238, 238, 238, 255],
                "uncovered edge leaked live magenta content"
            );
            for (x, y) in [(0, 0), (639, 0), (0, 419), (639, 419), (639, 200)] {
                assert_ne!(
                    pixels.get_pixel(x, y).0,
                    [255, 0, 255, 255],
                    "live magenta leaked through an unmasked edge"
                );
            }
            let white: Vec<_> = pixels
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0 == [255, 255, 255, 255])
                .map(|(x, _, _)| x)
                .collect();
            assert!(!white.is_empty(), "buffered paper was not retained");
            let middle = (*white.iter().min().unwrap() + *white.iter().max().unwrap()) as f64 / 2.0;
            assert!(
                (middle - 320.0).abs() <= 1.0,
                "scaled paper is not horizontally centered"
            );
            assert!(
                pixels.pixels().all(|p| p.0[3] == 255),
                "overlay has transparent holes"
            );
        }
        let hits = state.inspect()["hits"].as_u64().unwrap();
        for width in [560.0, 520.0, 480.0, 500.0] {
            web.set_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: wry::dpi::LogicalSize::new(width, 420.0).into(),
            })
            .unwrap();
            state.layout(&web);
        }
        let resize = state.inspect();
        assert_eq!(resize["hits"].as_u64().unwrap(), hits + 4);
        assert!(resize["in_flight"].is_null());
        assert_eq!(resize["deferred"], true);
        assert_eq!(resize["cached"], true);
        assert!((resize["transform"][0].as_f64().unwrap() - 0.8 * 500.0 / 640.0).abs() < 1e-12);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut vector_count = 0;
        loop {
            pump();
            while let Ok(message) = receiver.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&message).unwrap();
                capture_timings.record(&value);
                if value["type"] == "preview-zoom-snapshot" {
                    state.message(&web, &value, enabled);
                }
                if value["type"] == "vector" {
                    vector_count += 1;
                    assert_eq!(value["scale"], 0.8);
                }
            }
            if vector_count > 0 && state.inspect()["active"] == false {
                break;
            }
            assert!(Instant::now() < deadline, "resized image never handed off");
        }
        assert_eq!(
            vector_count, 1,
            "divider burst submitted intermediate vector work"
        );
        receipt["zoom_out"] = zoom_out;
        receipt["resize_during_block"] = resize;
        receipt["resize_vector_requests"] = serde_json::json!(vector_count);
    }
    receipt["actual_tinymist"] = serde_json::json!(live_url.is_some());
    receipt["partial"] = serde_json::json!(actual_partial);
    if let Some(path) = output.as_deref() {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    if !baseline
        && live_url.is_some()
        && std::env::args().any(|arg| arg == "--transition-regression")
    {
        let result = live_transition_regression(
            &web,
            &state,
            &receiver,
            &pump,
            &mut receipt,
            output.as_deref(),
            &mut capture_timings,
        );
        if let Some(path) = output.as_deref() {
            std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
        }
        if let Err(error) = result {
            state.allowed(false);
            window.orderOut(None);
            panic!("real Tinymist transition regression: {error}");
        }
    }
    receipt["capture_timings"] = capture_timings.inspect();
    if let Some(path) = output.as_deref() {
        std::fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    }
    println!("{receipt}");
    // A sibling responder must keep keys; hiding and disabling release the texture.
    if !baseline {
        assert!(window.makeFirstResponder(None));
        let before = state.inspect()["inputs"].clone();
        let event=NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,NSPoint::new(100.0,100.0),NSEventModifierFlags::Command,0.0,window.windowNumber(),None,&NSString::from_str("="),&NSString::from_str("="),false,24).unwrap();
        app.postEvent_atStart(&event, false);
        for _ in 0..10 {
            pump();
        }
        assert_eq!(state.inspect()["inputs"], before);
        state.allowed(false);
        assert_eq!(state.inspect()["cached"], false);
        assert_eq!(state.inspect()["active"], false);
    }
    if background {
        assert_ne!(
            frontmost_pid(),
            Some(std::process::id() as i32),
            "owned zoom events activated the test application"
        );
    }
    window.orderOut(None);
}

/// Test-only bounded spans; stores no SVG or document text.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct CaptureTimings {
    pending: Vec<(String, std::time::Instant)>,
    preparation_ms: Vec<f64>,
    host_spans: Vec<serde_json::Value>,
    capture_descriptors: Vec<serde_json::Value>,
    frontend: Vec<serde_json::Value>,
    last_frontend: Option<serde_json::Value>,
    native_frames: u64,
    dropped: u64,
}
#[cfg(target_os = "macos")]
impl CaptureTimings {
    fn record(&mut self, value: &serde_json::Value) {
        if value["type"] == "frontend-diagnostic"
            || value["type"] == "frontend-error"
            || value["type"] == "reset-error"
        {
            if self.frontend.len() < 16 {
                self.frontend.push(value.clone());
            } else {
                self.dropped += 1;
            }
            self.last_frontend = Some(value.clone());
            return;
        }
        if value["type"] == "buffer-preparation" {
            if let Some(ms) = value["ms"]
                .as_f64()
                .filter(|ms| ms.is_finite() && *ms >= 0.0)
            {
                if self.preparation_ms.len() < 32 {
                    self.preparation_ms.push(ms);
                } else {
                    self.dropped += 1;
                }
            }
            return;
        }
        if value["type"] != "preview-zoom-snapshot" {
            return;
        }
        if value["op"] == "frame" {
            self.native_frames += 1;
        }
        let Some(token) = value["token"].as_str() else {
            return;
        };
        match value["op"].as_str() {
            Some("capture") => {
                if self.capture_descriptors.len() == 8 {
                    self.capture_descriptors.remove(0);
                }
                self.capture_descriptors.push(serde_json::json!({
                    "token":token,"paper":value["geometry"]["paper"],
                    "scale":value["test_capture_descriptor"]["scale"],
                    "location":value["test_capture_descriptor"]["location"],
                    "source_page":value["geometry"]["pages"].as_array().and_then(|pages|pages.get(18)),
                    "source_scale":value["geometry"]["source_scale"],
                    "error":value["test_capture_descriptor"]["error"]
                }));
                if self.pending.len() == 4 {
                    self.pending.remove(0);
                    self.dropped += 1;
                }
                self.pending
                    .push((token.to_owned(), std::time::Instant::now()));
            }
            Some(outcome @ ("admit" | "reject")) => {
                if let Some(index) = self
                    .pending
                    .iter()
                    .position(|(pending, _)| pending == token)
                {
                    let (_, started) = self.pending.remove(index);
                    if self.host_spans.len() < 32 {
                        self.host_spans.push(serde_json::json!({"outcome":outcome,"ms":started.elapsed().as_secs_f64()*1000.0}));
                    } else {
                        self.dropped += 1;
                    }
                }
            }
            _ => {}
        }
    }
    fn descriptor(&self, native: &serde_json::Value) -> Option<&serde_json::Value> {
        self.capture_descriptors.iter().rev().find(|descriptor| {
            if let Some(token) = native["token"].as_str() {
                descriptor["token"] == token
            } else {
                // JavaScript serializes integral coordinates as JSON integers;
                // native Geometry serializes them as floats. Compare values,
                // rather than serde_json::Number representation variants.
                descriptor["paper"]
                    .as_array()
                    .zip(native["source_paper"].as_array())
                    .is_some_and(|(a, b)| {
                        a.len() == 4
                            && b.len() == 4
                            && a.iter().zip(b).all(|(a, b)| {
                                a.as_f64()
                                    .zip(b.as_f64())
                                    .is_some_and(|(a, b)| (a - b).abs() <= 1e-9)
                            })
                    })
                    && descriptor["scale"]
                        .as_f64()
                        .zip(native["base_scale"].as_f64())
                        .is_some_and(|(a, b)| (a - b).abs() < 1e-9)
            }
        })
    }
    fn inspect(&self) -> serde_json::Value {
        serde_json::json!({
            "svg_buffer_preparation_ms":self.preparation_ms,
            "capture_to_admit_or_reject_ms":self.host_spans,
            "pending":self.pending.len(),"dropped":self.dropped,"sample_cap":32,
            "native_frame_messages":self.native_frames,"frontend":self.frontend,"last_frontend":self.last_frontend,
            "capture_descriptors":self.capture_descriptors,"capture_descriptor_cap":8,
            "scope":"SVG preparation is synchronous WebContent wall time; capture-to-admit/reject includes native rasterization and callback/validation scheduling; neither is GPU presentation"
        })
    }
}

/// Injects the app's production zoom/layout operations into one owned window.
/// This does not synthesize trackpad magnification or measure GPU presentation.
#[cfg(target_os = "macos")]
fn live_transition_regression(
    web: &wry::WebView,
    state: &preview_zoom_snapshot::CaptureState,
    receiver: &std::sync::mpsc::Receiver<String>,
    pump: &impl Fn(),
    receipt: &mut serde_json::Value,
    output: Option<&std::path::Path>,
    capture_timings: &mut CaptureTimings,
) -> Result<(), String> {
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};
    use wry::WebViewExtMacOS;

    fn anchor_error(image: &Value, sharp: &Value) -> Option<[f64; 2]> {
        let source_anchor = &image["source_anchor"];
        let geometry = &sharp["geometry"];
        let actual = geometry["paper"].as_array()?;
        let scale = geometry["source_scale"]
            .as_f64()
            .filter(|scale| *scale > 0.0)?;
        let pages = geometry["pages"].as_array()?;
        let y = &source_anchor["y"];
        let projected = if let Some(page) = y["page"].as_u64() {
            pages.get(page as usize)?[0].as_f64()? + y["local"].as_f64()?
        } else {
            let before = pages.get(y["before"].as_u64()? as usize)?;
            let after = pages.get(y["after"].as_u64()? as usize)?;
            let end = before[0].as_f64()? + before[1].as_f64()?;
            end + y["fraction"].as_f64()? * (after[0].as_f64()? - end)
        };
        Some([
            actual[0].as_f64()? + source_anchor["x"].as_f64()? * scale
                - source_anchor["clientX"].as_f64()?,
            actual[1].as_f64()? + projected * scale - source_anchor["clientY"].as_f64()?,
        ])
    }

    struct Probe<'a, F: Fn()> {
        web: &'a wry::WebView,
        state: &'a preview_zoom_snapshot::CaptureState,
        receiver: &'a std::sync::mpsc::Receiver<String>,
        pump: &'a F,
        observation: Option<Value>,
        handoff: Option<Value>,
        request_id: u64,
        next_observation: Instant,
        blocked: bool,
        observation_timeout: Duration,
        capture_timings: &'a mut CaptureTimings,
    }
    impl<F: Fn()> Probe<'_, F> {
        fn tick(&mut self) {
            (self.pump)();
            for message in self.receiver.try_iter() {
                let value: Value = serde_json::from_str(&message).unwrap();
                self.capture_timings.record(&value);
                if value["type"] == "blocked" {
                    self.blocked = true;
                } else if value["type"] == "preview-zoom-snapshot" {
                    self.state.message(self.web, &value, true);
                } else if value["type"] == "zoom-observation" {
                    if value["label"] == "handoff" {
                        self.handoff = Some(value);
                    } else if value["request_id"].as_u64() == Some(self.request_id) {
                        self.observation = Some(value);
                    }
                }
            }
        }
        fn observe(&mut self, label: &str) -> Result<Value, String> {
            // Leave WebContent time to run its capture timers between expensive
            // geometry observations, while continuing native capture/IPC work.
            while Instant::now() < self.next_observation {
                self.tick();
            }
            self.observation = None;
            self.request_id += 1;
            self.web
                .evaluate_script(&format!(
                    "window.tiptoptypNativeZoomObserve({},{});",
                    serde_json::to_string(label).unwrap(),
                    self.request_id
                ))
                .map_err(|error| error.to_string())?;
            let deadline = Instant::now() + self.observation_timeout;
            loop {
                self.tick();
                if let Some(value) = &self.observation
                    && value["label"] == label
                    && value["request_id"].as_u64() == Some(self.request_id)
                {
                    let observation = value.clone();
                    self.next_observation = Instant::now() + Duration::from_millis(50);
                    return Ok(observation);
                }
                if Instant::now() > deadline {
                    return Err(format!("timed out observing {label}"));
                }
            }
        }
        fn idle_image(&mut self, label: &str, fresh: bool) -> Result<Value, String> {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let value = self.observe(label)?;
                let native = self.state.inspect();
                let geometry_matches = native["source_paper"]
                    .as_array()
                    .zip(value["geometry"]["paper"].as_array())
                    .is_some_and(|(cached, current)| {
                        cached.iter().zip(current).all(|(a, b)| {
                            a.as_f64()
                                .zip(b.as_f64())
                                .is_some_and(|(a, b)| (a - b).abs() < 1.0)
                        })
                    })
                    && native["base_scale"]
                        .as_f64()
                        .zip(value["scale"].as_f64())
                        .is_some_and(|(a, b)| (a - b).abs() < 1e-9);
                if value["ready"] == true
                    && native["cached"] == true
                    && native["active"] == false
                    && (!fresh || geometry_matches)
                {
                    return Ok(value);
                }
                if Instant::now() > deadline {
                    return Err(format!("no usable idle image for {label}: {native}"));
                }
            }
        }
        fn sharp(&mut self, previous_handoffs: u64, label: &str) -> Result<Value, String> {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                self.tick();
                let native = self.state.inspect();
                if native["handoffs"].as_u64().unwrap() > previous_handoffs
                    && native["active"] == false
                    && native["in_flight"].is_null()
                    && native["pending"].is_null()
                    && let Some(value) = &self.handoff
                    && value["ready"] == true
                    && value["request"]["revision"] == native["revision"]
                    && value["geometry"]["view"][0].as_f64().is_some_and(|width| {
                        (width - self.web.webview().bounds().size.width).abs() < 1.0
                    })
                    && value["geometry"]["view"][1].as_f64().is_some_and(|height| {
                        (height - self.web.webview().bounds().size.height).abs() < 1.0
                    })
                {
                    return Ok(value.clone());
                }
                if Instant::now() > deadline {
                    return Err(format!("no sharp handoff for {label}: {native}"));
                }
            }
        }
    }

    let dense_full_mode = receipt["partial"] == false;
    let reset_timeout = Duration::from_secs(if dense_full_mode { 20 } else { 8 });
    let mut probe = Probe {
        web,
        state,
        receiver,
        pump,
        observation: None,
        handoff: None,
        request_id: 0,
        next_observation: Instant::now(),
        blocked: false,
        observation_timeout: Duration::from_secs(if dense_full_mode { 15 } else { 4 }),
        capture_timings,
    };
    let mut failures = Vec::<String>::new();
    let mut steps = Vec::<Value>::new();
    let save = |steps: &[Value],
                failures: &[String],
                receipt: &mut Value,
                timings: &CaptureTimings| {
        receipt["capture_timings"] = timings.inspect();
        receipt["transition_regression"] = json!({
            "passed":failures.is_empty(),
            "input":"injected CaptureState zoom operations and owned WebView bounds; not physical pinch",
            "scope":"native image availability and cached-to-sharp geometry; excludes GPU presentation",
            "observation_cadence_ms":50,
            "viewport_start":[640,420],"mid_document_page":18,"mid_document_fraction":0.15,
            "steps":steps,"failures":failures
        });
        if let Some(path) = output {
            std::fs::write(path, serde_json::to_vec_pretty(receipt).unwrap()).unwrap();
        }
    };
    save(&steps, &failures, receipt, probe.capture_timings);
    let run = (|| -> Result<(), String> {
        web.evaluate_script(
            r#"void (async()=>{
                const doc=window.tiptoptypNativeZoomFrame().doc;
                const quiet=async predicate=>{
                    const deadline=performance.now()+(doc.partialRendering?6000:20000);
                    while(!predicate()){
                        if(performance.now()>deadline)throw Error('Timed out resetting native zoom fixture');
                        await new Promise(resolve=>setTimeout(resolve,20));
                    }
                };
                doc.clearSvgResizeAnchor();window.tiptoptypPreviewAction({action:'fit'});
                await quiet(()=>{
                    const frame=window.tiptoptypNativeZoomFrame();
                    return frame?.ready&&Math.abs(frame.scale-1)<1e-9&&!window.tiptoptypZoomSnapshot.inspect().active;
                });
                window.tiptoptypPreviewAction({action:'page',value:18});
                await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
                await quiet(()=>window.tiptoptypNativeZoomFrame()?.ready&&!window.tiptoptypZoomSnapshot.inspect().active);
                const scroll=doc.hookedElem.parentElement;
                const group=[...doc.hookedElem.firstElementChild.querySelectorAll('g.typst-page')][18];
                const matrix=group.getScreenCTM();
                scroll.scrollTo({left:0,top:matrix.f-scroll.getBoundingClientRect().top+scroll.scrollTop+
                    Number(group.getAttribute('data-page-height'))*matrix.d*.15,behavior:'instant'});
            })().catch(error=>window.ipc.postMessage(JSON.stringify({type:'reset-error',
                name:String(error?.name??'Error'),message:String(error?.message??error).slice(0,240),visibility:document.visibilityState})));"#,
        ).map_err(|error| error.to_string())?;
        // Observe the programmatic scroll before waiting for its idle capture;
        // the old page's already-admitted raster is not a valid reset result.
        let reset_deadline = Instant::now() + reset_timeout;
        loop {
            let reset = probe.observe("mid-document-reset")?;
            if reset["geometry"]["scroll"][1].as_f64().unwrap_or(0.0) > 1000.0
                && (reset["scale"].as_f64().unwrap_or(0.0) - 1.0).abs() < 1e-9
                && (reset["location"]["fraction"].as_f64().unwrap_or(0.0) - 0.15).abs() < 0.003
            {
                break;
            }
            if Instant::now() > reset_deadline {
                return Err("mid-document reset did not reach the selected page".into());
            }
        }
        let reset = probe.idle_image("mid-document-idle", true)?;
        steps.push(json!({"kind":"reset","frame":reset,"native":state.inspect()}));
        save(&steps, &failures, receipt, probe.capture_timings);

        // A sent resize can become obsolete while WebContent is busy. The
        // serial slot must still complete at the CURRENT fitted layout so its
        // queued replacement can run; a frozen dispatched width deadlocks it.
        let before_race = state.inspect();
        probe.blocked = false;
        web.evaluate_script("window.blockRenderer();")
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(4);
        while !probe.blocked {
            probe.tick();
            if Instant::now() > deadline {
                return Err("renderer did not start the owned stall".into());
            }
        }
        web.set_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(560., 420.).into(),
        })
        .map_err(|error| error.to_string())?;
        state.layout(web);
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.inspect()["in_flight"].is_null() {
            probe.tick();
            if Instant::now() > deadline {
                return Err("first resize was not dispatched during the stall".into());
            }
        }
        web.set_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(480., 420.).into(),
        })
        .map_err(|error| error.to_string())?;
        state.layout(web);
        let queued = state.inspect();
        if queued["pending"].is_null() {
            return Err("newer divider width was not queued behind the old flight".into());
        }
        let sharp = probe.sharp(
            before_race["handoffs"].as_u64().unwrap(),
            "resize-flight-race",
        )?;
        if sharp["geometry"]["view"][0].as_f64().unwrap() != 480. {
            return Err("serial resize completed at an obsolete viewport".into());
        }
        steps.push(json!({"kind":"resize-flight-race","widths":[640,560,480],"stall_ms":800,"before":before_race,"queued":queued,"sharp":sharp,"after":state.inspect()}));
        save(&steps, &failures, receipt, probe.capture_timings);
        let restore = state.inspect();
        web.set_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(640., 420.).into(),
        })
        .map_err(|error| error.to_string())?;
        state.layout(web);
        probe.sharp(
            restore["handoffs"].as_u64().unwrap(),
            "resize-flight-race-restore",
        )?;
        probe.idle_image("resize-flight-race-idle", true)?;

        // Actual WebKit callback acceptance is terminal too: a renderer reset
        // can refuse a request without any render acknowledgement. Verify the
        // native serial slot recovers, captures again, and accepts the next zoom.
        web.evaluate_script("window.acceptedNativeZoom=window.tiptoptypNativeZoom;window.tiptoptypNativeZoom=()=>false;")
            .map_err(|error|error.to_string())?;
        probe.observe("before-native-rejection")?;
        let before_reject = state.inspect();
        state.message(web, &json!({"op":"zoom","factor":1.01,"x":50,"y":50}), true);
        probe.idle_image("native-rejection-recovered", true)?;
        let rejected = state.inspect();
        if rejected["in_flight"].is_number()
            || rejected["pending"].is_number()
            || rejected["active"] == true
        {
            return Err("refused WebKit request left a native owner active".into());
        }
        web.evaluate_script("window.tiptoptypNativeZoom=window.acceptedNativeZoom;delete window.acceptedNativeZoom;")
            .map_err(|error|error.to_string())?;
        probe.observe("restored-native-renderer")?;
        state.message(web, &json!({"op":"zoom","factor":1.01,"x":50,"y":50}), true);
        let recovered = probe.sharp(
            rejected["handoffs"].as_u64().unwrap(),
            "native-rejection-next-zoom",
        )?;
        steps.push(json!({"kind":"native-rejection-recovery","before":before_reject,"rejected":rejected,"next_zoom":recovered,"after":state.inspect()}));
        save(&steps, &failures, receipt, probe.capture_timings);
        state.message(
            web,
            &json!({"op":"zoom","factor":1,"reset":true,"x":50,"y":50}),
            true,
        );
        probe.idle_image("native-rejection-restored-fit", true)?;

        // Opposite off-center positions prove both proxy and sharp SVG preserve
        // the source point under the pointer. Divider input later discards this
        // pinch anchor and preserves the viewport's top instead.
        for (point, factor) in [([56.0, 330.0], 1.04), ([590.0, 80.0], 1.0 / 1.04)] {
            probe.idle_image("before-off-center-zoom", false)?;
            let before = state.inspect();
            state.message(
                web,
                &json!({"op":"zoom","factor":factor,"x":point[0],"y":point[1]}),
                true,
            );
            let image = state.inspect();
            let source = &image["source_anchor"];
            if source["clientX"].as_f64() != Some(point[0])
                || source["clientY"].as_f64() != Some(point[1])
            {
                failures.push(format!("off-center zoom used a different anchor: {source}"));
            }
            let shifted = probe.sharp(before["handoffs"].as_u64().unwrap(), "off-center-zoom")?;
            let error =
                anchor_error(&image, &shifted).ok_or("missing off-center anchor geometry")?;
            if error.iter().any(|value| value.abs() > 1.1) {
                failures.push(format!(
                    "off-center zoom jumped under pointer {point:?}: {error:?}"
                ));
            }
            if image["active"] != true {
                failures.push("off-center zoom missed the idle raster".into());
            }
            steps.push(json!({"kind":"off-center-zoom","point":point,"image":image,"frame":shifted,"anchor_error_css_pixels":error,"native":state.inspect()}));
            save(&steps, &failures, receipt, probe.capture_timings);
        }
        probe.idle_image("before-real-divider", false)?;

        // Deterministically deliver the duplicate DOM notification between the
        // sharp DOM mutation and its next paint/ready callback. AppKit already
        // owns these bounds; the notification must not strand the native flight.
        web.evaluate_script(
            r#"const originalNativeZoom=window.tiptoptypNativeZoom;
            window.tiptoptypNativeZoom=request=>{
                const accepted=originalNativeZoom(request);
                if(accepted&&request.resize)requestAnimationFrame(()=>window.dispatchEvent(new Event('resize')));
                return accepted;
            };"#,
        )
        .map_err(|error| error.to_string())?;

        if std::env::args().any(|arg| arg == "--scroll-race") {
            // WebKit can clamp scroll before delivering its resize notification.
            // Exercise that order over real DOM geometry and the native owner.
            web.evaluate_script(r#"const resizeSnapshot=window.tiptoptypZoomSnapshot.resize;
                let delivering=false;
                window.tiptoptypZoomSnapshot.resize=()=>{
                    if(!delivering){delivering=true;try{window.tiptoptypZoomSnapshot.scrolled();}finally{delivering=false;}}
                    return resizeSnapshot();
                };"#).map_err(|error|error.to_string())?;
        }

        for width in [560.0, 640.0] {
            let before = state.inspect();
            let before_frame = probe
                .handoff
                .clone()
                .ok_or("missing pre-divider sharp ink observation")?;
            let started = Instant::now();
            web.set_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: wry::dpi::LogicalSize::new(width, 420.0).into(),
            })
            .map_err(|error| error.to_string())?;
            state.layout(web);
            let image = state.inspect();
            let feedback_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            let image_hit = image["hits"].as_u64().unwrap() > before["hits"].as_u64().unwrap();
            let descriptor = probe
                .capture_timings
                .descriptor(&image)
                .cloned()
                .ok_or("missing admitted raster page-18 descriptor")?;
            let transform = image["transform"]
                .as_array()
                .ok_or("missing native transform")?;
            let paper = image["source_paper"]
                .as_array()
                .ok_or("divider discarded admitted geometry")?;
            let ratio = transform[0].as_f64().unwrap();
            let expected = [
                paper[0].as_f64().unwrap() * ratio + transform[1].as_f64().unwrap(),
                paper[1].as_f64().unwrap() * ratio + transform[2].as_f64().unwrap(),
                paper[2].as_f64().unwrap() * ratio,
            ];
            let sharp = probe.sharp(before["handoffs"].as_u64().unwrap(), "real-divider")?;
            let actual = sharp["geometry"]["paper"]
                .as_array()
                .ok_or("missing sharp divider geometry")?;
            let error = [
                actual[0].as_f64().unwrap() - expected[0],
                actual[1].as_f64().unwrap() - expected[1],
                actual[2].as_f64().unwrap() - expected[2],
            ];
            let geometry = &sharp["geometry"];
            let anchor_error = anchor_error(&image, &sharp);
            let scroll_error = (|| -> Option<[f64; 2]> {
                if !geometry["pages"].as_array()?.is_empty() {
                    return None;
                }
                Some([
                    geometry["scroll"][0].as_f64()? - image["target_scroll"]["left"].as_f64()?,
                    geometry["scroll"][1].as_f64()? - image["target_scroll"]["top"].as_f64()?,
                ])
            })();
            let source_pages = image["source_pages"].as_array();
            let source_page = source_pages.and_then(|pages| pages.get(18)).or_else(|| {
                (!descriptor["source_page"].is_null()).then_some(&descriptor["source_page"])
            });
            let piecewise = source_page.is_some();
            let page_ink = &descriptor["location"];
            let expected_ink = if let Some(page) = source_page {
                let scale = image["source_scale"]
                    .as_f64()
                    .or_else(|| descriptor["source_scale"].as_f64())
                    .ok_or("missing captured page scale")?;
                [
                    paper[1].as_f64().unwrap() * ratio
                        + transform[2].as_f64().unwrap()
                        + page[0].as_f64().ok_or("missing source page offset")? * scale * ratio
                        + 18.0 * 10.0 * (1.0 - ratio),
                    page[1].as_f64().ok_or("missing source page height")? * scale * ratio,
                ]
            } else {
                [
                    page_ink["top"]
                        .as_f64()
                        .ok_or("missing captured page top")?
                        * ratio
                        + transform[2].as_f64().unwrap(),
                    page_ink["height"]
                        .as_f64()
                        .ok_or("missing captured page height")?
                        * ratio,
                ]
            };
            let actual_ink = [
                sharp["location"]["top"]
                    .as_f64()
                    .ok_or("missing sharp page top")?,
                sharp["location"]["height"]
                    .as_f64()
                    .ok_or("missing sharp page height")?,
            ];
            let ink_error = [
                actual_ink[0] - expected_ink[0],
                actual_ink[1] - expected_ink[1],
            ];
            let top_source_error = (sharp["location"]["fraction"]
                .as_f64()
                .ok_or("missing sharp page fraction")?
                - before_frame["location"]["fraction"]
                    .as_f64()
                    .ok_or("missing initial page fraction")?)
                * actual_ink[1];
            // A root paper rectangle can match while page ink moves, because
            // physical page gaps don't scale. Check the admitted raster's actual
            // page projection and the source point at the viewport top.
            if error[0].abs() > 1.1
                || error[2].abs() > 1.1
                || (!piecewise && error[1].abs() > 1.1)
                || ink_error.iter().any(|error| error.abs() > 1.1)
                || top_source_error.abs() > 1.1
                || anchor_error.is_some_and(|error| error.iter().any(|error| error.abs() > 1.1))
                || scroll_error.is_some_and(|error| error.iter().any(|error| error.abs() > 1.1))
            {
                failures.push(format!("{width}-point divider handoff moved content: paper={error:?}, ink={ink_error:?}, top-source={top_source_error}, anchor={anchor_error:?}, scroll={scroll_error:?}"));
            }
            if page_ink["ink"] != true || sharp["location"]["ink"] != true {
                failures.push(format!(
                    "{width}-point divider lacked captured/current page-18 ink"
                ));
            }
            if image["hits"].as_u64().unwrap() <= before["hits"].as_u64().unwrap() {
                failures.push(format!(
                    "{width}-point divider did not use the retained raster"
                ));
            }
            steps.push(json!({"kind":"divider","width":width,"before":before,"image":image,
                "sharp":sharp,"expected_paper_xy_width":expected,"paper_error_css_pixels":error,
                "anchor_error_css_pixels":anchor_error,"scroll_error_css_pixels":scroll_error,
                "native_geometry_observer":{"source_anchor":!image["source_anchor"].is_null(),"target_scroll":!image["target_scroll"].is_null()},
                "capture_descriptor":descriptor,"before_frame":before_frame,"piecewise":piecewise,
                "expected_page18_ink_top_height":expected_ink,"actual_page18_ink_top_height":actual_ink,
                "ink_error_css_pixels":ink_error,"viewport_top_source_error_css_pixels":top_source_error,
                "first_input_to_cached_submission_us":image_hit.then_some(feedback_us),
                "input_to_sharp_ms":started.elapsed().as_secs_f64()*1000.0,"after":state.inspect()}));
            save(&steps, &failures, receipt, probe.capture_timings);
        }

        // Only the first gesture gets an idle-image wait. Every following one
        // starts at the sharp handoff, inside recapture/admission's vulnerable gap.
        probe.idle_image("before-successive-gestures", false)?;
        for (index, factor) in [1.05, 1.0 / 1.05, 1.05].into_iter().enumerate() {
            let before = state.inspect();
            let started = Instant::now();
            state.message(
                web,
                &json!({"op":"zoom","factor":factor,"x":315,"y":195}),
                true,
            );
            let feedback_us = started.elapsed().as_secs_f64() * 1_000_000.0;
            let during = state.inspect();
            if during["hits"].as_u64().unwrap() <= before["hits"].as_u64().unwrap()
                || during["cached"] != true
            {
                failures.push(format!(
                    "successive gesture {index} had no immediate cached feedback"
                ));
            }
            let sharp = probe.sharp(before["handoffs"].as_u64().unwrap(), "successive-gesture")?;
            steps.push(json!({"kind":"successive-gesture","index":index,"factor":factor,
                "before":before,"during":during,"feedback_us":feedback_us,
                "first_input_to_cached_submission_us":(during["hits"].as_u64().unwrap()>before["hits"].as_u64().unwrap()).then_some(feedback_us),
                "input_to_sharp_ms":started.elapsed().as_secs_f64()*1000.0,"sharp":sharp,"after":state.inspect()}));
            save(&steps, &failures, receipt, probe.capture_timings);
        }

        // A temporary excursion outside the cached paper must hide the proxy
        // and request sharp work, while keeping its pixels for a quick return.
        probe.idle_image("before-coverage-excursion", false)?;
        let before = state.inspect();
        let started = Instant::now();
        state.message(
            web,
            &json!({"op":"zoom","factor":0.25,"x":315,"y":195}),
            true,
        );
        let uncovered = state.inspect();
        if uncovered["active"] != false || uncovered["in_flight"].is_null() {
            failures.push("out-of-buffer zoom did not immediately use sharp fallback".into());
        }
        if uncovered["cached"] != true || uncovered["source_paper"] != before["source_paper"] {
            failures
                .push("out-of-buffer zoom permanently discarded reusable pixels/geometry".into());
        }
        let return_started = Instant::now();
        state.message(
            web,
            &json!({"op":"zoom","factor":4.0,"x":315,"y":195}),
            true,
        );
        let returned = state.inspect();
        let return_feedback_us = return_started.elapsed().as_secs_f64() * 1_000_000.0;
        if returned["hits"].as_u64().unwrap() <= uncovered["hits"].as_u64().unwrap()
            || returned["cached"] != true
        {
            failures.push("return into the cached buffer failed to reuse its raster".into());
        }
        steps.push(
            json!({"kind":"coverage-excursion","before":before,"uncovered":uncovered,
            "returned":returned,"excursion_and_return_submission_us":started.elapsed().as_secs_f64()*1_000_000.0,
            "first_input_to_cached_submission_us":(returned["hits"].as_u64().unwrap()>uncovered["hits"].as_u64().unwrap()).then_some(return_feedback_us)}),
        );
        save(&steps, &failures, receipt, probe.capture_timings);
        let sharp = probe.sharp(before["handoffs"].as_u64().unwrap(), "coverage-return")?;
        steps.push(json!({"kind":"coverage-return-sharp","frame":sharp,"native":state.inspect()}));
        save(&steps, &failures, receipt, probe.capture_timings);
        if std::env::args().any(|arg| arg == "--scroll-race") {
            for edge in ["top", "bottom"] {
                web.evaluate_script(&format!(
                    "{{window.tiptoptypZoomSnapshot.invalidate();const s=window.tiptoptypNativeZoomFrame().doc.hookedElem.parentElement;s.scrollTo(0,{});window.tiptoptypZoomSnapshot.settle();}}",
                    if edge == "top" {0} else {100_000_000})).map_err(|e|e.to_string())?;
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    let observed = probe.observe("edge-reset-observation")?;
                    let top = observed["geometry"]["scroll"][1]
                        .as_f64()
                        .ok_or("missing actual edge scroll")?;
                    let target = if edge == "top" {
                        0.0
                    } else {
                        observed["scroll_limits"][1]
                            .as_f64()
                            .ok_or("missing real browser scroll limit")?
                    };
                    if (top - target).abs() < 1.0 {
                        break;
                    }
                    if Instant::now() > deadline {
                        return Err(format!(
                            "did not reach actual {edge} edge: {top} vs {target}"
                        ));
                    }
                }
                probe.idle_image("edge-reset", true)?;
                for factor in [0.95, 1.2, 1.0 / 1.14] {
                    let before = state.inspect();
                    let started = Instant::now();
                    state.message(
                        web,
                        &json!({"op":"zoom","factor":factor,"x":630,"y":400}),
                        true,
                    );
                    let image = state.inspect();
                    if edge == "bottom"
                        && factor == 0.95
                        && std::env::args().any(|arg| arg == "--dark")
                        && let Some(path) = std::env::var_os("TIPTOPTYP_NATIVE_ZOOM_PNG")
                    {
                        let path = std::path::PathBuf::from(path).with_extension("edge.png");
                        state.write_overlay_png(&path);
                        let pixels = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
                        let ratio = image["transform"][0].as_f64().unwrap();
                        let right = image["source_paper"][0].as_f64().unwrap() * ratio
                            + image["transform"][1].as_f64().unwrap()
                            + image["source_paper"][2].as_f64().unwrap() * ratio;
                        let x = (right - 8.0)
                            .floor()
                            .clamp(0.0, (pixels.width() - 1) as f64)
                            as u32;
                        let pixel = pixels.get_pixel(x, pixels.height() / 2).0;
                        if pixel != [82, 86, 89, 255] {
                            failures.push(format!(
                                "cached right gutter contains {pixel:?}, expected opaque live gray"
                            ));
                        }
                        if pixels
                            .pixels()
                            .any(|p| p.0[0] > 245 && p.0[1] > 245 && p.0[2] > 245)
                        {
                            failures
                                .push("unexpected white pixels in the dark cached preview".into());
                        }
                    }
                    let sharp = probe.sharp(before["handoffs"].as_u64().unwrap(), "edge-sharp")?;
                    let ratio = image["transform"][0].as_f64().unwrap();
                    let expected_top = image["source_paper"][1].as_f64().unwrap() * ratio
                        + image["transform"][2].as_f64().unwrap();
                    let error = sharp["geometry"]["paper"][1].as_f64().unwrap() - expected_top;
                    if error.abs() > 1.1 {
                        failures.push(format!("{edge} zoom {factor} jumped {error}px"));
                    }
                    if image["active"] != true {
                        failures.push(format!("{edge} zoom {factor} discarded warmed pixels"));
                    }
                    steps.push(json!({"kind":"edge","edge":edge,"factor":factor,"before":before,"image":image,"sharp":sharp,"error":error,"input_to_sharp_ms":started.elapsed().as_secs_f64()*1000.0}));
                    save(&steps, &failures, receipt, probe.capture_timings);
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = run {
        failures.push(error);
    }
    if probe
        .capture_timings
        .frontend
        .iter()
        .any(|value| value["type"] == "frontend-error" || value["type"] == "reset-error")
    {
        failures.push("frontend exception during native transition regression".into());
    }
    save(&steps, &failures, receipt, probe.capture_timings);
    receipt["transition_regression"]["latest_observation"] = serde_json::json!(probe.observation);
    receipt["transition_regression"]["last_handoff"] = serde_json::json!(probe.handoff);
    receipt["transition_regression"]["final_native"] = state.inspect();
    if let Some(path) = output {
        std::fs::write(path, serde_json::to_vec_pretty(receipt).unwrap()).unwrap();
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
