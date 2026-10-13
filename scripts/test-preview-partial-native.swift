// Opt-in standalone WKWebView evidence, driven by test-preview-partial-native.py.
// This deliberately does not claim to observe the composed editor window.
import AppKit
import WebKit

struct Configuration: Decodable {
    let url: String
    let adapter: String
    let result: String
    let snapshots: String
    let mode: String
    let warmupSeconds: Double
    let measuredSamples: Int
    let pageCount: Int
    let phaseDiagnostics: Bool?
    let maxSvgNodes: Int?
    let snapshotBuffer: String?
    let baselineBuffer: String?
}

@MainActor
final class Probe: NSObject, NSApplicationDelegate {
    let configuration: Configuration
    var window: NSWindow!
    var web: WKWebView!
    var deadline: Date!
    var observations = [[String: Any]]()
    var transitions = [[String: Any]]()
    var samples = [[String: Any]]()
    var failures = [String]()
    var idleBefore = [String: Any]()
    var initialBefore = [String: Any]()

    init(_ configuration: Configuration) {
        self.configuration = configuration
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let settings = WKWebViewConfiguration()
        // Real app WebKit views are actively scheduled while their parent is visible.
        if #available(macOS 14.0, *) { settings.preferences.inactiveSchedulingPolicy = .none }
        do {
            if let baseline = configuration.baselineBuffer {
                let source = try String(contentsOfFile: baseline, encoding: .utf8)
                settings.userContentController.addUserScript(WKUserScript(
                    source: source + "\nwindow.tttBaselineBuffer=window.tiptoptypSnapshotBuffer;",
                    injectionTime: .atDocumentStart, forMainFrameOnly: true))
            }
            if let buffer = configuration.snapshotBuffer {
                settings.userContentController.addUserScript(WKUserScript(
                    source: try String(contentsOfFile: buffer, encoding: .utf8),
                    injectionTime: .atDocumentStart, forMainFrameOnly: true))
            }
            let adapter = try String(contentsOfFile: configuration.adapter, encoding: .utf8)
            settings.userContentController.addUserScript(WKUserScript(source: adapter,
                injectionTime: .atDocumentStart, forMainFrameOnly: true))
            settings.userContentController.addUserScript(WKUserScript(source: Self.observation,
                injectionTime: .atDocumentStart, forMainFrameOnly: true))
        } catch { finish(error: "Cannot read adapter: \(error.localizedDescription)"); return }
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 900, height: 700), configuration: settings)
        window = NSWindow(contentRect: web.frame, styleMask: [.titled, .closable],
            backing: .buffered, defer: false)
        window.title = "Tinymist \(configuration.mode) renderer — standalone native probe"
        let parent = NSView(frame: web.frame)
        window.contentView = parent
        parent.addSubview(web)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(web)
        NSApplication.shared.activate(ignoringOtherApps: true)
        deadline = Date().addingTimeInterval(20)
        web.load(URLRequest(url: URL(string: configuration.url)!))
        ready()
        DispatchQueue.main.asyncAfter(deadline: .now() + 120) {
            self.finish(error: "Native probe exceeded its 120-second deadline")
        }
    }

    func evaluate(_ body: String, then: @escaping ([String: Any]) -> Void) {
        web.callAsyncJavaScript(body, arguments: [:], in: nil, in: .page) { result in
            switch result {
            case .success(let value):
                guard let text = value as? String,
                      let data = text.data(using: .utf8),
                      let decoded = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                    self.finish(error: "Native JavaScript result was not a JSON object"); return
                }
                then(decoded)
            case .failure(let error): self.finish(error: "Native JavaScript failed: \(error.localizedDescription)")
            }
        }
    }

    func ready() {
        evaluate("return JSON.stringify(window.tttNativeObserve?.() ?? {});") { state in
            if (state["pages"] as? Int) == self.configuration.pageCount {
                self.initialBefore = state
                DispatchQueue.main.asyncAfter(deadline: .now() + self.configuration.warmupSeconds) {
                    self.observe("initial") {
                        self.snapshot("initial", target: 0) {
                            if self.configuration.snapshotBuffer != nil { self.measureBuffer() }
                            else { self.jump(0) }
                        }
                    }
                }
            } else if Date() < self.deadline {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { self.ready() }
            } else { self.finish(error: "Pinned renderer did not initialize all page skeletons") }
        }
    }

    func validate(_ state: [String: Any], name: String) {
        let partial = configuration.mode == "partial"
        if (state["partial"] as? Bool) != partial { failures.append("\(name): effective renderer mode differs from requested mode") }
        if (state["pages"] as? Int) != configuration.pageCount { failures.append("\(name): page skeleton count changed") }
        let visible = state["visible"] as? [[String: Any]] ?? []
        if visible.isEmpty { failures.append("\(name): no visible page") }
        for page in visible {
            if (page["dummy"] as? Bool) == true || (page["inkNodes"] as? Int ?? 0) == 0 {
                failures.append("\(name): visible page \(page["index"] ?? "unknown") has no rendered SVG ink")
            }
        }
        if partial {
            if (state["canvases"] as? Int ?? 0) != 0 || (state["canvasMixins"] as? Int ?? 0) != 0 {
                failures.append("\(name): partial renderer allocated fallback canvases")
            }
            if (state["populated"] as? Int ?? 0) > visible.count + 6 {
                failures.append("\(name): partial renderer populated more than its fixture window bound")
            }
            if (state["nodes"] as? Int ?? 0) > (configuration.maxSvgNodes ?? 30000) {
                failures.append("\(name): partial renderer exceeded fixture DOM bound")
            }
        }
    }

    func observe(_ name: String, then: @escaping () -> Void) {
        evaluate("return JSON.stringify(window.tttNativeObserve());") { state in
            self.validate(state, name: name)
            var observation = state
            observation["name"] = name
            self.observations.append(observation)
            then()
        }
    }

    func jump(_ stage: Int) {
        let targets = [17, 42, 3, 47, 0]
        guard stage < targets.count else { beginSamples(); return }
        let target = targets[stage]
        evaluate("""
        const doc = document.getElementById('typst-container').documents[0].impl;
        const scroll = doc.hookedElem.parentElement;
        const page = [...doc.hookedElem.firstElementChild.children].filter(node => node.matches('g.typst-page'))[\(target)];
        const matrix = page.getScreenCTM();
        doc.clearSvgResizeAnchor();
        scroll.scrollTo({top:matrix.f - scroll.getBoundingClientRect().top + scroll.scrollTop + Number(page.getAttribute('data-page-height'))*matrix.d*.15,behavior:'instant'});
        const states = [];
        for(let frame=0;frame<2;frame++) { await new Promise(requestAnimationFrame); states.push(window.tttNativeObserve()); }
        return JSON.stringify({states});
        """) { result in
            let states = result["states"] as? [[String: Any]] ?? []
            for (frame, state) in states.enumerated() {
                self.validate(state, name: "jump-\(target)-RAF-\(frame + 1)")
                self.transitions.append(["name": "jump-\(target)", "frame": frame + 1, "state": state])
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                self.observe("jump-\(target)") {
                    if target == 42 {
                        self.snapshot("far-page-42", target: target) { self.jump(stage + 1) }
                    } else { self.jump(stage + 1) }
                }
            }
        }
    }

    func snapshot(_ name: String, target: Int, then: @escaping () -> Void) {
        web.takeSnapshot(with: nil) { image, error in
            guard let data = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data),
                  let png = bitmap.representation(using: .png, properties: [:]) else {
                self.finish(error: "WebKit snapshot unavailable: \(String(describing: error))"); return
            }
            let path = URL(fileURLWithPath: self.configuration.snapshots, isDirectory: true)
                .appendingPathComponent("\(self.configuration.mode)-\(name).png")
            do { try png.write(to: path) } catch { self.finish(error: "Cannot save WebKit snapshot"); return }
            let color = [36 + (target * 37) % 180, 30 + (target * 73) % 180, 20 + (target * 47) % 180]
            var ink = 0
            for y in stride(from: 0, to: bitmap.pixelsHigh, by: 3) {
                for x in stride(from: 0, to: bitmap.pixelsWide, by: 3) {
                    var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
                    bitmap.getPixel(&pixel, atX: x, y: y)
                    if zip(pixel.prefix(3), color).allSatisfy({ abs($0 - $1) <= 3 }) { ink += 1 }
                }
            }
            if ink <= 10 { self.failures.append("\(name): page-specific ink missing from WebKit snapshot") }
            self.observations.append(["name": "snapshot-\(name)", "path": path.path,
                "pixelWidth": bitmap.pixelsWide, "pixelHeight": bitmap.pixelsHigh, "markerSamples": ink])
            then()
        }
    }

    func beginSamples() {
        evaluate("""
        const doc = document.getElementById('typst-container').documents[0].impl;
        const scroll = doc.hookedElem.parentElement;
        const group = [...doc.hookedElem.firstElementChild.children].filter(node => node.matches('g.typst-page'))[18];
        const matrix = group.getScreenCTM();
        doc.clearSvgResizeAnchor();
        scroll.scrollTo({top:matrix.f-scroll.getBoundingClientRect().top+scroll.scrollTop+Number(group.getAttribute('data-page-height'))*matrix.d*.15,behavior:'instant'});
        window.tttNativeProbe = {active:false};
        for(const name of ['rescale','rerender']) {
          const original = doc.r[name];
          doc.r[name] = function(...args) {
            if(!window.tttNativeProbe.active) return original.apply(this,args);
            const before=performance.now();
            try { return original.apply(this,args); }
            finally { const stats=window.tttNativeProbe; stats[name]++;stats[name+'SyncMs']+=performance.now()-before; }
          };
        }
        const original=doc.addViewportChange;
        doc.addViewportChange=function(...args) { if(window.tttNativeProbe.active)window.tttNativeProbe.viewport++;return original.apply(this,args); };
        return JSON.stringify({instrumented:true});
        """) { _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + self.configuration.warmupSeconds) { self.sample(-1) }
        }
    }

    func sample(_ index: Int) {
        guard index < configuration.measuredSamples else { verifyIdle(); return }
        // Resize/zoom anchoring can finish at a different scroll position even
        // when a workload's wheel deltas cancel. Reset every sample explicitly.
        web.setFrameSize(NSSize(width: 900, height: 700))
        evaluate("""
        const doc=document.getElementById('typst-container').documents[0].impl;
        const scroll=doc.hookedElem.parentElement;
        doc.clearSvgResizeAnchor();
        window.tiptoptypPreviewAction({action:'fit'});
        await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
        doc.clearSvgResizeAnchor();
        const group=[...doc.hookedElem.firstElementChild.children].filter(node=>node.matches('g.typst-page'))[18];
        const matrix=group.getScreenCTM();
        scroll.scrollTo({left:0,top:matrix.f-scroll.getBoundingClientRect().top+scroll.scrollTop+Number(group.getAttribute('data-page-height'))*matrix.d*.15,behavior:'instant'});
        let previousState=window.tttNativeObserve(),stableFrames=0;
        for(let attempt=0;attempt<12 && stableFrames<2;attempt++) {
          await new Promise(requestAnimationFrame);
          const next=window.tttNativeObserve();
          const same=next.viewport?.[0]===900 && next.viewport?.[1]===700 &&
            Math.abs(next.scale-1)<1e-6 && next.page===18 && Math.abs(next.pageFraction-.15)<.001 &&
            Math.abs(next.scrollTop-previousState.scrollTop)<.5;
          stableFrames=same?stableFrames+1:0;previousState=next;
        }
        if(stableFrames<2)throw Error('Sample reset did not reach the fixed stable page/scale/viewport');
        window.tttNativeStartSample(\(configuration.phaseDiagnostics == true ? "true" : "false"));
        return JSON.stringify({started:true});
        """) { _ in self.resize(index, step: 0) }
    }

    func resize(_ sample: Int, step: Int) {
        guard step < 36 else { scrollAndZoom(sample); return }
        let width = 900 - (250 * sin(Double(step) / 35 * Double.pi)).rounded()
        web.setFrameSize(NSSize(width: width, height: 700))
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.02) { self.resize(sample, step: step + 1) }
    }

    func scrollAndZoom(_ sample: Int) {
        evaluate("""
        const doc=document.getElementById('typst-container').documents[0].impl;
        const scroll=doc.hookedElem.parentElement,start=scroll.scrollTop;
        window.tttNativeMarkPhase('scroll');
        for(let step=0;step<24;step++) {
          scroll.scrollTo({top:start+3000*Math.sin(step/23*Math.PI),behavior:'instant'});
          await new Promise(resolve=>setTimeout(resolve,20));
        }
        // The same quiet interval in both lanes admits the native cache ahead
        // of the gesture. Capture misses are measured separately from warm hits.
        await new Promise(resolve=>setTimeout(resolve,1000));
        window.tttNativeMarkPhase('zoom');
        let lastZoomInput=0;
        for(let step=0;step<10;step++) {
          lastZoomInput=performance.now();
          doc.hookedElem.dispatchEvent(new WheelEvent('wheel',{bubbles:true,cancelable:true,ctrlKey:true,
            deltaY:step<5?-10:10,clientX:350,clientY:200}));
          await new Promise(resolve=>setTimeout(resolve,20));
        }
        for(let frame=0;frame<120;frame++) {
          await new Promise(requestAnimationFrame);
          if(!doc.isRendering && !window.tiptoptypZoomSnapshot?.inspect().active)break;
        }
        const zoomFinalRenderMs=performance.now()-lastZoomInput;
        window.tttNativeMarkPhase('drain');
        await new Promise(resolve=>setTimeout(resolve,800));
        window.tttNativeClosePhase();
        const stats=window.tttNativeProbe;
        window.tttNativeStopSample(stats);
        const frames=[...stats.frameGaps].sort((a,b)=>a-b);
        const result={zoomFinalRenderMs,workloadWallMs:performance.now()-stats.started,rescale:stats.rescale,rerender:stats.rerender,
          viewport:stats.viewport,rescaleSyncMs:stats.rescaleSyncMs,rerenderSyncMs:stats.rerenderSyncMs,
          frameCount:frames.length,frameGapP95Ms:frames[Math.floor(frames.length*.95)]??0,
          frameGapMaxMs:frames.at(-1)??0,frameSamplesDropped:stats.frameSamplesDropped,startState:stats.startState,endState:window.tttNativeObserve()};
        if(stats.phaseDiagnostics)result.phases=stats.phases;
        return JSON.stringify(result);
        """) { result in
            if sample >= 0 { var receipt = result; receipt["sample"] = sample; self.samples.append(receipt) }
            self.sample(sample + 1)
        }
    }

    // Opt-in synchronous preparation cost, outside resize/zoom measurements.
    // Keep DOM validation/serialization comparisons outside the timed section.
    func measureBuffer() {
        evaluate("""
        const doc=document.getElementById('typst-container').documents[0].impl;
        window.tiptoptypPreviewAction({action:'page',value:18});
        await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);
        await new Promise(resolve=>setTimeout(resolve,1000));
        const svg=doc.hookedElem.firstElementChild,scroll=doc.hookedElem.parentElement;
        const frame={svg,width:innerWidth,height:innerHeight,background:[255,255,255]};
        const before=[svg.outerHTML,scroll.scrollLeft,scroll.scrollTop],times=[];
        let buffer;
        for(let index=-1;index<\(configuration.measuredSamples);index++) {
          const start=performance.now();buffer=window.tiptoptypSnapshotBuffer(frame);
          const ms=performance.now()-start;if(index>=0)times.push(ms);
          await new Promise(resolve=>setTimeout(resolve,50));
        }
        if(!buffer)throw Error('Buffered snapshot unavailable');
        const parsed=new DOMParser().parseFromString(buffer.html,'text/html');
        const canonical=html=> {
          const tree=new DOMParser().parseFromString(html,'text/html'),defs=tree.querySelector('defs');
          defs.replaceChildren(...[...defs.children].sort((a,b)=>a.outerHTML.localeCompare(b.outerHTML)));
          return tree.documentElement.outerHTML;
        };
        const equivalent=!window.tttBaselineBuffer || canonical(buffer.html)===canonical(window.tttBaselineBuffer(frame).html);
        const ink=[...parsed.querySelectorAll('g.typst-page')];
        const hrefs=[...parsed.querySelectorAll('use')].map(node=>node.getAttribute('href')||node.getAttribute('xlink:href')).filter(href=>href?.startsWith('#'));
        const dependencies=hrefs.every(href=>!document.getElementById(href.slice(1))||parsed.getElementById(href.slice(1)));
        const unchanged=before[0]===svg.outerHTML&&before[1]===scroll.scrollLeft&&before[2]===scroll.scrollTop;
        return JSON.stringify({times,htmlBytes:buffer.html.length,inkPages:ink.length,
          definitions:parsed.querySelector('defs').children.length,nodes:parsed.querySelectorAll('*').length,
          dependencies,unchanged,equivalent,scripts:parsed.scripts.length,
          populated:ink.every(node=>node.getAttribute('data-dummy')!=='1'&&node.querySelector('use,path,text')),
          sourceNodes:svg.querySelectorAll('*').length,state:window.tttNativeObserve()});
        """) { result in
            for key in ["dependencies", "unchanged", "populated", "equivalent"] {
                if result[key] as? Bool != true { self.failures.append("Buffered snapshot failed \(key)") }
            }
            if let state = result["state"] as? [String: Any] { self.validate(state, name: "buffer") }
            self.samples.append(result)
            self.finish(error: nil)
        }
    }

    func verifyIdle() {
        observe("measurement-end") {
            self.web.setFrameSize(NSSize(width: 900, height: 700))
            self.evaluate("window.tiptoptypPreviewAction({action:'fit'});return JSON.stringify({fit:true});") { _ in
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                    self.evaluate("""
                    window.tttNativeStartIdle();
                    return JSON.stringify(window.tttNativeObserve());
                    """) { state in
                        self.idleBefore = state
                        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                            self.evaluate("""
                            const idle=window.tttNativeFinishIdle();
                            return JSON.stringify({...idle,state:window.tttNativeObserve(),errors:window.tttNativeErrors});
                            """) { result in
                                if (result["quiet"] as? Bool) != true {
                                    self.failures.append("Idle renderer kept requesting work")
                                }
                                if !(result["errors"] as? [String] ?? []).isEmpty { self.failures.append("WebKit reported JavaScript errors") }
                                if let state = result["state"] as? [String: Any] { self.validate(state, name: "idle") }
                                self.observations.append(["name": "idle", "result": result])
                                self.finish(error: nil)
                            }
                        }
                    }
                }
            }
        }
    }

    func finish(error: String?) {
        if let error { failures.append(error) }
        let receipt: [String: Any] = ["mode": configuration.mode, "passed": failures.isEmpty,
            "viewSize": [900, 700], "backingScaleFactor": window?.backingScaleFactor ?? 0,
            "warmupSeconds": configuration.warmupSeconds, "workloadWarmups": 1,
            "measuredSamples": configuration.measuredSamples, "samples": samples,
            "initial": initialBefore, "observations": observations, "transientFrames": transitions,
            "idleBefore": idleBefore, "failures": failures,
            "evidence": "Standalone native WKWebView; snapshots do not prove composed editor geometry or input routing"]
        do {
            let data = try JSONSerialization.data(withJSONObject: receipt, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: configuration.result))
        } catch { print("Cannot write native result: \(error.localizedDescription)"); exit(2) }
        print("Native \(configuration.mode) probe: \(failures.isEmpty ? "passed" : "failed") — \(configuration.result)")
        exit(failures.isEmpty ? 0 : 1)
    }

    static let observation = """
    window.tttNativeErrors=[];
    for(const type of ['error','unhandledrejection'])window.addEventListener(type,event=> {
      const value=type==='error'?event.error:event.reason;
      const knownNames=new Set(['Error','TypeError','RangeError','ReferenceError','SyntaxError','URIError','EvalError']);
      const name=knownNames.has(value?.name)?value.name:'unknown';
      if(window.tttNativeErrors.length<32)window.tttNativeErrors.push(`${type}:${name}`);
    });
    window.tttNativeCounters=()=>({active:true,rescale:0,rerender:0,viewport:0,rescaleSyncMs:0,rerenderSyncMs:0});
    window.tttNativeStopSample=(stats=window.tttNativeProbe)=> {
      if(!stats)return;
      stats.active=false;
      if(stats.frame)cancelAnimationFrame(stats.frame);
      stats.frame=0;
    };
    window.tttNativeStartSample=phaseDiagnostics=> {
      window.tttNativeStopSample();
      const stats=window.tttNativeProbe={...window.tttNativeCounters(),frameGaps:[],frameSamplesDropped:0,
        started:performance.now(),startState:window.tttNativeObserve(),phaseDiagnostics};
      if(stats.phaseDiagnostics) { stats.phases=[];window.tttNativeMarkPhase('resize'); }
      let previous=null;
      stats.frame=requestAnimationFrame(function frame(now) {
        // A callback already dispatched when stopped must not revive after
        // a later sample or idle check switches the active counter object.
        if(!stats.active || window.tttNativeProbe!==stats)return;
        if(previous!==null) {
          if(stats.frameGaps.length<2048)stats.frameGaps.push(now-previous);else stats.frameSamplesDropped++;
        }
        previous=now;stats.frame=requestAnimationFrame(frame);
      });
      return stats;
    };
    window.tttNativeStartIdle=()=> {
      window.tttNativeStopSample();
      // Idle counts renderer work without starting or reactivating a sampler.
      return window.tttNativeProbe=window.tttNativeCounters();
    };
    window.tttNativeFinishIdle=()=> {
      const stats=window.tttNativeProbe;
      window.tttNativeStopSample(stats);
      return {rescale:stats.rescale,rerender:stats.rerender,viewport:stats.viewport,
        quiet:stats.rescale===0 && stats.rerender===0 && stats.viewport===0};
    };
    // Phase summaries reuse the one bounded sample RAF buffer. A gap belongs
    // to the phase active when its ending callback arrives; no second sampler.
    window.tttNativeClosePhase=()=> {
      const stats=window.tttNativeProbe,phase=stats?.phase;
      if(!stats?.phaseDiagnostics || !phase)return;
      const finished=performance.now();
      const frames=stats.frameGaps.slice(phase.frameOffset).sort((a,b)=>a-b);
      stats.phases.push({phase:phase.name,durationMs:finished-phase.started,
        rescale:stats.rescale-phase.rescale,rerender:stats.rerender-phase.rerender,
        viewport:stats.viewport-phase.viewport,rescaleSyncMs:stats.rescaleSyncMs-phase.rescaleSyncMs,
        rerenderSyncMs:stats.rerenderSyncMs-phase.rerenderSyncMs,
        frameCount:frames.length,frameGapP95Ms:frames[Math.floor(frames.length*.95)]??0,
        frameGapMaxMs:frames.at(-1)??0,frameSamplesDropped:stats.frameSamplesDropped-phase.droppedOffset});
      stats.phase=null;
    };
    window.tttNativeMarkPhase=name=> {
      const stats=window.tttNativeProbe;
      if(!stats?.phaseDiagnostics)return;
      window.tttNativeClosePhase();
      const order=['resize','scroll','zoom','drain'];
      if(stats.phases.length>=4 || name!==order[stats.phases.length])throw Error('Invalid bounded native phase order');
      stats.phase={name,started:performance.now(),frameOffset:stats.frameGaps.length,droppedOffset:stats.frameSamplesDropped,
        rescale:stats.rescale,rerender:stats.rerender,viewport:stats.viewport,
        rescaleSyncMs:stats.rescaleSyncMs,rerenderSyncMs:stats.rerenderSyncMs};
    };
    window.tttNativeObserve=()=> {
      const doc=document.getElementById('typst-container')?.documents?.[0]?.impl;
      const svg=doc?.hookedElem?.firstElementChild,scroll=doc?.hookedElem?.parentElement;
      if(!svg || !scroll || svg.tagName.toLowerCase()!=='svg')return {};
      const viewport=scroll.getBoundingClientRect();
      const pages=[...svg.children].filter(node=>node.matches('g.typst-page'));
      const visible=[];let populated=0,dominant=null;
      pages.forEach((group,index)=> {
        const dummy=group.getAttribute('data-dummy')==='1';if(!dummy)populated++;
        const matrix=group.getScreenCTM(),width=Number(group.getAttribute('data-page-width')),height=Number(group.getAttribute('data-page-height'));
        if(matrix && Math.min(matrix.f+height*matrix.d,viewport.bottom)-Math.max(matrix.f,viewport.top)>8 &&
           Math.min(matrix.e+width*matrix.a,viewport.right)-Math.max(matrix.e,viewport.left)>8) {
          const overlap=Math.min(matrix.f+height*matrix.d,viewport.bottom)-Math.max(matrix.f,viewport.top);
          const page={index,dummy,inkNodes:group.querySelectorAll('use,path,text,rect:not(.typst-page-inner)').length};
          visible.push(page);
          if(!dominant || overlap>dominant.overlap)dominant={index,overlap,fraction:(viewport.top-matrix.f)/(height*matrix.d)};
        }
      });
      return {pages:pages.length,populated,visible,nodes:svg.querySelectorAll('*').length,
        canvases:svg.querySelectorAll('canvas').length,canvasMixins:svg.querySelectorAll('[data-mixin-canvas]').length,
        zoomSnapshot:window.tiptoptypZoomSnapshot?.inspect(),partial:doc.partialRendering,scale:doc.currentScaleRatio,scrollTop:scroll.scrollTop,
        page:dominant?.index??-1,pageFraction:dominant?.fraction??null,
        viewport:[scroll.clientWidth,scroll.clientHeight]};
    };
    """
}

MainActor.assumeIsolated {
    do {
        guard CommandLine.arguments.count == 2 else { throw NSError(domain: "NativeProbe", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Supply the runner's JSON configuration path"] ) }
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let configuration = try JSONDecoder().decode(Configuration.self, from: data)
        guard ["full", "partial"].contains(configuration.mode), configuration.pageCount > 1,
              (1...320000).contains(configuration.maxSvgNodes ?? 30000),
              (1...10).contains(configuration.measuredSamples), (0...15).contains(configuration.warmupSeconds),
              let url = URL(string: configuration.url), url.host == "127.0.0.1" else {
            throw NSError(domain: "NativeProbe", code: 2,
                userInfo: [NSLocalizedDescriptionKey: "Invalid bounded probe configuration"])
        }
        let app = NSApplication.shared
        let probe = Probe(configuration)
        app.delegate = probe
        app.setActivationPolicy(.accessory)
        app.run()
    } catch { print("Cannot start native probe: \(error.localizedDescription)"); exit(2) }
}
