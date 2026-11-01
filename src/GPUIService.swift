import Cocoa
import Darwin
import CryptoKit
import UserNotifications

// AppKit companion owns macOS integration; GPUI owns every application window.
private struct HeatWorkloadObservation {
    let key: String
    let label: String
    let cpu: Double
    let workers: Int
    let workerNoun: String
}

private struct HeatStatsSample {
    let date: Date
    let processCPU: [String: Double]
    let workloads: [HeatWorkloadObservation]
}

private struct HeatCandidate {
    let key: String
    let label: String
    let averageCPU: Double
    let workers: Int
    let workerNoun: String?
    let isWorkload: Bool
}

private struct HeatAggregate {
    var label: String
    var totalCPU: Double
    var maxWorkers: Int
    var workerNoun: String
}

final class GPUIService: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private let heatNotificationCategory = "BATTERY_HOG_HEAT"
    private let heatOpenAction = "BATTERY_HOG_HEAT_OPEN_WORKLOADS"
    private let heatSnoozeAction = "BATTERY_HOG_HEAT_SNOOZE"
    private let heatPermissionRequestedKey = "HeatWatchNotificationPermissionRequested"
    private let heatLastAlertKey = "HeatWatchLastAlertAt"
    private let heatSnoozeUntilKey = "HeatWatchSnoozeUntil"
    private let heatHistoryWindow: TimeInterval = 90
    private let heatCooldown: TimeInterval = 15 * 60

    private var backend: BatteryHogBackend!
    private var hostBundle: Bundle {
        ProcessInfo.processInfo.environment["BATTERY_HOG_APP_BUNDLE"].flatMap { Bundle(path: $0) } ?? .main
    }
    private var appDefaults: UserDefaults {
        hostBundle.bundleIdentifier.flatMap { UserDefaults(suiteName: $0) } ?? .standard
    }
    var statusItem: NSStatusItem!
    var pollTimer: Timer?
    var lastLowPower: Bool? = nil
    private var statsPollInFlight = false
    private var heatHistory: [HeatStatsSample] = []
    private var currentThermalState: ProcessInfo.ThermalState = .nominal
    private var heatElevatedAt: Date?
    private var heatEpisodeArmed = true
    private var heatEvaluationWorkItem: DispatchWorkItem?
    private var heatAlertsEnabled = false

    func applicationDidFinishLaunching(_ note: Notification) {
        // Read the initial value before subscribing, as recommended for
        // ProcessInfo state-change notifications: the notification is only a
        // signal to fetch the new value, not a container for the state itself.
        let initialThermalState = ProcessInfo.processInfo.thermalState
        currentThermalState = debugThermalStateOverride() ?? initialThermalState
        configureHeatNotifications()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(thermalStateDidChange(_:)),
            name: ProcessInfo.thermalStateDidChangeNotification,
            object: nil
        )
        applyThermalState(currentThermalState, at: Date())

        backend = BatteryHogBackend(configuration: .live(bundle: hostBundle))
        buildStatusItem()
        startInputLoop()
        emit(["event": "ready", "languages": Localization.preferredLanguages])
        startStatsPolling()
    }

    func buildStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let b = statusItem.button {
            let cfg = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
            let img = NSImage(systemSymbolName: "bolt.fill", accessibilityDescription: "Battery Hog")?
                .withSymbolConfiguration(cfg)
            img?.isTemplate = true
            b.image = img
            b.imagePosition = .imageLeading
        }
        let menu = NSMenu()
        menu.addItem(NSMenuItem(title: L("Starting…"), action: nil, keyEquivalent: ""))
        statusItem.menu = menu
    }

    private func startStatsPolling() {
        if pollTimer == nil {
            pollTimer = Timer.scheduledTimer(withTimeInterval: 7.0, repeats: true) { [weak self] _ in
                self?.pollStats()
            }
        }
        pollStats()
    }

    func pollStats() {
        guard !statsPollInFlight else { return }
        statsPollInFlight = true
        backend.stats { [weak self] obj in
            DispatchQueue.main.async {
                guard let self else { return }
                self.statsPollInFlight = false
                self.applyStats(obj)
            }
        }
    }

    func applyStats(_ obj: [String: Any]) {
        let procs = obj["processes"] as? [[String: Any]] ?? []
        let workloads = obj["workloads"] as? [[String: Any]] ?? []
        lastLowPower = obj["lowpower"] as? Bool
        let battery = obj["battery"] as? [String: Any] ?? [:]
        let power = obj["power"] as? [String: Any] ?? [:]
        let dev = obj["dev_summary"] as? [String: Any] ?? [:]
        let pct = battery["percent"] as? Int
        let watts = power["watts"] as? Double
        let direction = power["direction"] as? String ?? ""
        let onAC = (battery["on_ac"] as? Bool) ?? false
        let timeStr = battery["time"] as? String

        // status-bar title: user-configurable via Settings ▸ Menu bar
        let settings = obj["settings"] as? [String: Any] ?? [:]
        let heat = settings["heat"] as? [String: Any] ?? [:]
        recordHeatStats(processes: procs, workloads: workloads, at: Date())
        updateHeatAlertsEnabled((heat["enabled"] as? Bool) ?? false)
        evaluateHeatEpisode(at: Date())
        let mb = settings["menubar"] as? [String: Any] ?? [:]
        func mbOn(_ k: String, _ def: Bool) -> Bool { (mb[k] as? Bool) ?? def }
        var titleParts: [String] = []
        if mbOn("percent", true), let pct = pct { titleParts.append("\(pct)%") }
        if mbOn("watts", true), let w = watts, direction != "ac" {
            titleParts.append("\(Int(w.rounded()))W")
        }
        if mbOn("time", false), let t = timeStr, direction == "charging" || direction == "discharging" {
            titleParts.append(t)
        }
        if mbOn("hog", false), let top = procs.first, let name = top["name"] as? String {
            var short = name
            if short.count > 12 { short = String(short.prefix(11)) + "…" }
            let cpu = Int((top["cpu"] as? Double) ?? 0)
            titleParts.append("\(short) \(cpu)%")
        }
        if mbOn("dev", false) {
            let projects = dev["projects"] as? Int ?? 0
            let heavy = dev["heavy_workers"] as? Int ?? 0
            if projects > 0 { titleParts.append(L("{jobs} jobs · {projects} dev", ["jobs": String(heavy), "projects": String(projects)])) }
        }
        statusItem.button?.title = titleParts.isEmpty ? "" : " " + titleParts.joined(separator: " · ")

        emit(["event": "stats", "body": withIcons(obj)])

        // rebuild the menu
        let menu = NSMenu()

        // battery summary line
        var parts: [String] = []
        if let pct = pct { parts.append("\(pct)%") }
        parts.append(onAC ? (direction == "charging" ? L("Charging") : L("Plugged in")) : L("On battery"))
        if let w = watts { parts.append(String(format: "%.1f W", w)) }
        if let t = timeStr, direction == "charging" || direction == "discharging" {
            parts.append(t + (direction == "charging" ? L(" to full") : L(" left")))
        }
        let summary = NSMenuItem(title: parts.joined(separator: "  ·  "), action: nil, keyEquivalent: "")
        summary.isEnabled = false
        menu.addItem(summary)
        menu.addItem(.separator())

        let hdr = NSMenuItem(title: L("Apps by estimated impact"), action: nil, keyEquivalent: "")
        hdr.isEnabled = false
        menu.addItem(hdr)

        for p in procs.prefix(6) {
            guard let name = p["name"] as? String else { continue }
            let cpu = Int((p["cpu"] as? Double) ?? 0)
            let memMB = (p["mem_mb"] as? Double) ?? 0
            let memStr = memMB >= 1024 ? String(format: "%.1f GB", memMB / 1024) : "\(Int(memMB)) MB"
            let protectedFlag = (p["protected"] as? Bool) ?? false
            let item = NSMenuItem(title: "\(name)   \(cpu)% · \(memStr)", action: nil, keyEquivalent: "")
            if protectedFlag {
                item.isEnabled = false
            } else {
                let sub = NSMenu()
                let q = NSMenuItem(title: L("Quit {name}", ["name": name]), action: #selector(quitProcess(_:)), keyEquivalent: "")
                q.target = self
                q.representedObject = ["name": name,
                                       "path": p["path"] as? String ?? "",
                                       "bundle": (p["bundle"] as? Bool) ?? false,
                                       "pids": p["pids"] ?? []]
                sub.addItem(q)
                item.submenu = sub
            }
            menu.addItem(item)
        }

        let projects = dev["projects"] as? Int ?? 0
        if projects > 0 {
            menu.addItem(.separator())
            let workers = dev["active_workers"] as? Int ?? 0
            let heavy = dev["heavy_workers"] as? Int ?? 0
            let item = NSMenuItem(title: L("Development   {projects} projects · {workers} active · {heavy} heavy", ["projects": String(projects), "workers": String(workers), "heavy": String(heavy)]),
                                  action: #selector(showWorkloads(_:)), keyEquivalent: "")
            item.target = self
            menu.addItem(item)
        }

        menu.addItem(.separator())
        let lpm = NSMenuItem(title: L("Low Power Mode"), action: #selector(toggleLPM(_:)), keyEquivalent: "")
        lpm.target = self
        lpm.state = (lastLowPower == true) ? .on : .off
        if lastLowPower == nil { lpm.isEnabled = false }
        menu.addItem(lpm)

        menu.addItem(.separator())
        let show = NSMenuItem(title: L("Open Dashboard"), action: #selector(showDashboard(_:)), keyEquivalent: "")
        show.target = self
        menu.addItem(show)
        let quit = NSMenuItem(title: L("Quit Battery Hog"), action: #selector(quitGUI(_:)), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        statusItem.menu = menu
    }

    @objc func quitProcess(_ sender: NSMenuItem) {
        guard let d = sender.representedObject as? [String: Any],
              let name = d["name"] as? String else { return }
        let a = NSAlert()
        a.messageText = L("Quit {name}?", ["name": name])
        a.informativeText = L("It will be asked to close — save your work first.")
        a.addButton(withTitle: L("Quit"))
        a.addButton(withTitle: L("Cancel"))
        if a.runModal() == .alertFirstButtonReturn {
            backendPOST("/api/kill", ["name": name,
                                      "path": d["path"] ?? "",
                                      "bundle": d["bundle"] ?? false,
                                      "pids": d["pids"] ?? []])
        }
    }

    @objc func toggleLPM(_ sender: NSMenuItem) {
        let turnOn = !(lastLowPower ?? false)
        backendPOST("/api/lowpower", ["on": turnOn])
    }


    @objc func showDashboard(_ sender: Any?) { emit(["event": "open", "room": "overview"]) }
    @objc func showWorkloads(_ sender: Any?) { emit(["event": "open", "room": "workloads"]) }
    @objc func quitGUI(_ sender: Any?) { emit(["event": "quit"]); NSApp.terminate(nil) }
    // MARK: Heat Watch

    private func configureHeatNotifications() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let open = UNNotificationAction(identifier: heatOpenAction,
                                        title: L("Open Workloads"), options: [.foreground])
        let snooze = UNNotificationAction(identifier: heatSnoozeAction,
                                          title: L("Snooze 1 hour"), options: [])
        let category = UNNotificationCategory(identifier: heatNotificationCategory,
                                              actions: [open, snooze],
                                              intentIdentifiers: [], options: [])
        center.setNotificationCategories([category])
    }

    private var isPreviewBuild: Bool {
        hostBundle.bundleIdentifier?.hasSuffix(".preview") == true
    }

    /// Preview-only deterministic hooks for testing without heating a Mac:
    ///   BATTERY_HOG_DEBUG_THERMAL_STATE=fair|serious|critical
    ///   BATTERY_HOG_DEBUG_HEAT_ENABLED=1
    ///   BATTERY_HOG_DEBUG_HEAT_DELAY_SECONDS=0
    ///   BATTERY_HOG_DEBUG_HEAT_DRY_RUN=1 (logs the notification, does not post it)
    /// They are deliberately ignored by release-bundle identifiers.
    private func debugEnvironment(_ key: String) -> String? {
        guard isPreviewBuild else { return nil }
        return ProcessInfo.processInfo.environment[key]
    }

    private func debugThermalStateOverride() -> ProcessInfo.ThermalState? {
        switch debugEnvironment("BATTERY_HOG_DEBUG_THERMAL_STATE")?.lowercased() {
        case "nominal": return .nominal
        case "fair": return .fair
        case "serious": return .serious
        case "critical": return .critical
        default: return nil
        }
    }

    private var heatSustainDelay: TimeInterval {
        guard let raw = debugEnvironment("BATTERY_HOG_DEBUG_HEAT_DELAY_SECONDS"),
              let delay = Double(raw) else { return 10 }
        return max(0, delay)
    }

    private var heatDebugDryRun: Bool {
        debugEnvironment("BATTERY_HOG_DEBUG_HEAT_DRY_RUN") == "1"
    }

    private func updateHeatAlertsEnabled(_ settingEnabled: Bool) {
        let enabled = settingEnabled || debugEnvironment("BATTERY_HOG_DEBUG_HEAT_ENABLED") == "1"
        let becameEnabled = enabled && !heatAlertsEnabled
        heatAlertsEnabled = enabled
        if becameEnabled {
            requestHeatNotificationPermissionIfNeeded()
            scheduleHeatEvaluationIfNeeded(at: Date())
            evaluateHeatEpisode(at: Date())
        }
    }

    private func requestHeatNotificationPermissionIfNeeded() {
        // A dry run intentionally avoids a system prompt; all normal enabled
        // Heat Watch sessions request permission exactly once per bundle.
        if heatDebugDryRun { return }
        let defaults = appDefaults
        guard !defaults.bool(forKey: heatPermissionRequestedKey) else { return }
        defaults.set(true, forKey: heatPermissionRequestedKey)
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
            granted, error in
            if let error = error {
                NSLog("Battery Hog Heat Watch: notification permission failed: \(error)")
            } else if !granted {
                NSLog("Battery Hog Heat Watch: notification permission was not granted")
            }
        }
    }

    @objc private func thermalStateDidChange(_ notification: Notification) {
        let state = debugThermalStateOverride() ?? ProcessInfo.processInfo.thermalState
        DispatchQueue.main.async { [weak self] in
            self?.applyThermalState(state, at: Date())
        }
    }

    private func applyThermalState(_ state: ProcessInfo.ThermalState, at now: Date) {
        currentThermalState = state
        if state == .nominal {
            heatElevatedAt = nil
            heatEpisodeArmed = true
            heatEvaluationWorkItem?.cancel()
            heatEvaluationWorkItem = nil
            return
        }
        if heatElevatedAt == nil {
            heatElevatedAt = now
        }
        scheduleHeatEvaluationIfNeeded(at: now)
    }

    private func scheduleHeatEvaluationIfNeeded(at now: Date) {
        guard currentThermalState != .nominal,
              heatEpisodeArmed,
              let elevatedAt = heatElevatedAt else { return }
        heatEvaluationWorkItem?.cancel()
        let remaining = max(0, heatSustainDelay - now.timeIntervalSince(elevatedAt))
        let item = DispatchWorkItem { [weak self] in
            self?.evaluateHeatEpisode(at: Date())
        }
        heatEvaluationWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + remaining, execute: item)
    }

    private func evaluateHeatEpisode(at now: Date) {
        guard heatAlertsEnabled,
              currentThermalState != .nominal,
              heatEpisodeArmed,
              let elevatedAt = heatElevatedAt else { return }
        let elapsed = now.timeIntervalSince(elevatedAt)
        guard elapsed >= heatSustainDelay else {
            scheduleHeatEvaluationIfNeeded(at: now)
            return
        }

        // One evaluation per elevated-temperature episode. Cool/nominal is the
        // only condition that re-arms it, including when cooldown or snooze wins.
        heatEpisodeArmed = false
        heatEvaluationWorkItem?.cancel()
        heatEvaluationWorkItem = nil

        let defaults = appDefaults
        let snoozeUntil = defaults.double(forKey: heatSnoozeUntilKey)
        if snoozeUntil > now.timeIntervalSince1970 { return }
        let lastAlert = defaults.double(forKey: heatLastAlertKey)
        if lastAlert > 0 && now.timeIntervalSince1970 - lastAlert < heatCooldown { return }

        let body = heatNotificationBody(at: now)
        defaults.set(now.timeIntervalSince1970, forKey: heatLastAlertKey)
        postHeatNotification(body: body, at: now)
    }

    private func recordHeatStats(processes: [[String: Any]], workloads: [[String: Any]], at now: Date) {
        var processCPU: [String: Double] = [:]
        for process in processes {
            // System/protected helpers are not actionable explanations and can
            // include Battery Hog's own short-lived sampling commands.
            if (process["protected"] as? Bool) == true { continue }
            guard let name = process["name"] as? String, !name.isEmpty else { continue }
            let cpu = (process["cpu"] as? NSNumber)?.doubleValue ?? 0
            if cpu > 0 { processCPU[name, default: 0] += cpu }
        }

        var workloadObservations: [HeatWorkloadObservation] = []
        for (index, workload) in workloads.enumerated() {
            let cpu = (workload["cpu"] as? NSNumber)?.doubleValue ?? 0
            guard cpu > 0 else { continue }
            let project = (workload["name"] as? String) ?? "Development"
            let key = (workload["id"] as? String) ?? "workload-\(index)-\(project)"
            let tools = workload["tools"] as? [[String: Any]] ?? []
            let families = workload["families"] as? [String] ?? []
            let agents = workload["agents"] as? [String] ?? []
            let tool = tools.first?["name"] as? String
            let label = heatWorkloadLabel(project: project, agent: agents.first,
                                          tool: tool, families: families)
            let activeWorkers = (workload["active_workers"] as? NSNumber)?.intValue ?? 0
            let workers = activeWorkers > 0
                ? activeWorkers
                : ((workload["workers"] as? NSNumber)?.intValue ?? 0)
            let status = (workload["status"] as? String) ?? "Active"
            workloadObservations.append(HeatWorkloadObservation(
                key: key, label: label, cpu: cpu, workers: workers,
                workerNoun: heatWorkerNoun(for: status)
            ))
        }

        heatHistory.append(HeatStatsSample(date: now, processCPU: processCPU,
                                           workloads: workloadObservations))
        let cutoff = now.addingTimeInterval(-heatHistoryWindow)
        heatHistory.removeAll { $0.date < cutoff }
    }

    private func heatWorkloadLabel(project: String, agent: String?, tool: String?,
                                   families: [String]) -> String {
        // When an agent/editor is the ancestor of the workers, it is the most
        // recognizable and actionable attribution for the person using the Mac.
        if let agent = agent, !agent.isEmpty { return agent }
        if let tool = tool, !tool.isEmpty {
            let genericTools = ["Node", "Node task", "Java", "Native build", "Development"]
            if !genericTools.contains(tool) { return tool }
        }
        if let family = families.first, !family.isEmpty {
            return L("{project} {family} workload", ["project": project, "family": family])
        }
        return L("{project} development workload", ["project": project])
    }

    private func normalizedHeatContributorName(_ label: String) -> String {
        let compact = label.lowercased().filter { $0.isLetter || $0.isNumber }
        if compact.contains("visualstudiocode") || compact.contains("vscode") ||
            compact.contains("codehelper") { return "vscode" }
        if compact.contains("codex") { return "codex" }
        if compact.contains("cursor") { return "cursor" }
        if compact.contains("claude") { return "claude" }
        if compact.contains("xcode") { return "xcode" }
        return compact
    }

    private func heatWorkerNoun(for status: String) -> String {
        switch status.lowercased() {
        case "compiling": return L("compiler workers")
        case "building": return L("build workers")
        case "testing": return L("test workers")
        case "serving": return L("dev-server workers")
        case "scanning": return L("scan workers")
        case "installing": return L("install workers")
        default: return L("development workers")
        }
    }

    private func heatCandidates(at now: Date) -> [HeatCandidate] {
        let cutoff = now.addingTimeInterval(-heatHistoryWindow)
        let samples = heatHistory.filter { $0.date >= cutoff }
        guard !samples.isEmpty else { return [] }

        var processTotals: [String: Double] = [:]
        var workloadTotals: [String: HeatAggregate] = [:]
        for sample in samples {
            for (name, cpu) in sample.processCPU {
                processTotals[name, default: 0] += cpu
            }
            for workload in sample.workloads {
                var aggregate = workloadTotals[workload.key] ?? HeatAggregate(
                    label: workload.label, totalCPU: 0, maxWorkers: 0,
                    workerNoun: workload.workerNoun
                )
                aggregate.label = workload.label
                aggregate.totalCPU += workload.cpu
                aggregate.maxWorkers = max(aggregate.maxWorkers, workload.workers)
                aggregate.workerNoun = workload.workerNoun
                workloadTotals[workload.key] = aggregate
            }
        }

        let divisor = Double(samples.count)
        var candidates = workloadTotals.map { key, value in
            HeatCandidate(key: "workload:\(key)", label: value.label,
                          averageCPU: value.totalCPU / divisor,
                          workers: value.maxWorkers, workerNoun: value.workerNoun,
                          isWorkload: true)
        }

        // Workload discovery already includes these executables. Avoid counting
        // a compiler once as a project workload and again as a raw process.
        let devExecutables = Set([
            "swift", "swiftc", "swift-frontend", "xcodebuild", "rustc", "cargo",
            "sccache", "gradle", "gradlew", "java", "kotlinc", "go", "pytest",
            "jest", "vitest", "playwright", "cypress", "tsc", "esbuild", "webpack",
            "rollup", "turbo", "npm", "pnpm", "yarn", "bun", "npx", "node",
            "make", "cmake", "ninja"
        ])
        let hasWorkloads = !workloadTotals.isEmpty
        for (name, total) in processTotals {
            let normalized = name.lowercased()
            if normalized.contains("battery_hog") || normalized == "battery hog" { continue }
            if hasWorkloads && devExecutables.contains(normalized) { continue }
            candidates.append(HeatCandidate(key: "process:\(name)", label: name,
                                            averageCPU: total / divisor, workers: 0,
                                            workerNoun: nil, isWorkload: false))
        }
        return candidates.filter { $0.averageCPU >= 1 }
            .sorted { left, right in
                if left.averageCPU == right.averageCPU { return left.isWorkload && !right.isWorkload }
                return left.averageCPU > right.averageCPU
            }
    }

    private func heatNotificationBody(at now: Date) -> String {
        let candidates = heatCandidates(at: now)
        guard let top = candidates.first else {
            return L("Thermal pressure has stayed elevated. Open Workloads to see what is active.")
        }

        let topCPU = Int(top.averageCPU.rounded())
        var body = L("Likely {name}", ["name": top.label])
        if let workerNoun = top.workerNoun, top.workers > 0 {
            body += L(": {count} {workers} averaging {cpu}% CPU.", ["count": String(top.workers), "workers": workerNoun, "cpu": String(topCPU)])
        } else {
            body += L(", averaging {cpu}% CPU.", ["cpu": String(topCPU)])
        }
        let normalizedTop = normalizedHeatContributorName(top.label)
        if let second = candidates.dropFirst().first(where: {
            $0.averageCPU >= 15 && $0.averageCPU >= top.averageCPU * 0.15
                && normalizedHeatContributorName($0.label) != normalizedTop
        }) {
            body += L(" {name} is also contributing at {cpu}% CPU.", ["name": second.label, "cpu": String(Int(second.averageCPU.rounded()))])
        }
        return body
    }

    private func postHeatNotification(body: String, at now: Date) {
        if heatDebugDryRun {
            NSLog("%@", "Battery Hog Heat Watch dry run — Your Mac is heating up — \(body)")
            return
        }
        let content = UNMutableNotificationContent()
        content.title = L("Your Mac is heating up")
        content.body = body
        content.sound = .default
        content.categoryIdentifier = heatNotificationCategory
        let request = UNNotificationRequest(
            identifier: "battery-hog-heat-\(Int(now.timeIntervalSince1970))",
            content: content, trigger: nil
        )
        UNUserNotificationCenter.current().add(request) { error in
            if let error = error {
                NSLog("Battery Hog Heat Watch: notification failed: \(error)")
            }
        }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler:
                                @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        switch response.actionIdentifier {
        case heatOpenAction, UNNotificationDefaultActionIdentifier:
            DispatchQueue.main.async { [weak self] in self?.showWorkloads(nil) }
        case heatSnoozeAction:
            appDefaults.set(Date().addingTimeInterval(60 * 60).timeIntervalSince1970,
                            forKey: heatSnoozeUntilKey)
        default:
            break
        }
        completionHandler()
    }

    func backendPOST(_ path: String, _ body: [String: Any]) {
        backend.post(path: path, body: body) { [weak self] in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { self?.pollStats() }
        }
    }


    private var iconPaths = [String: String]()
    private func withIcons(_ stats: [String: Any]) -> [String: Any] {
        var result = stats
        let directory = BatteryHogConfiguration.live(bundle: hostBundle).dataDirectory.appendingPathComponent("icons")
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        result["processes"] = (stats["processes"] as? [[String: Any]] ?? []).enumerated().map { index, process in
            var process = process
            guard index < 40, process["bundle"] as? Bool == true,
                  let path = process["path"] as? String, !path.isEmpty else { return process }
            if iconPaths[path] == nil {
                let name = SHA256.hash(data: Data(path.utf8)).map { String(format: "%02x", $0) }.joined()
                let file = directory.appendingPathComponent(name + ".png")
                let image = NSWorkspace.shared.icon(forFile: path)
                let bitmap = NSImage(size: NSSize(width: 64, height: 64))
                bitmap.lockFocus()
                image.draw(in: NSRect(x: 0, y: 0, width: 64, height: 64))
                bitmap.unlockFocus()
                if let data = bitmap.tiffRepresentation.flatMap({ NSBitmapImageRep(data: $0) })?.representation(using: .png, properties: [:]) {
                    try? data.write(to: file, options: .atomic)
                    iconPaths[path] = file.path
                }
            }
            process["icon_path"] = iconPaths[path]
            return process
        }
        return result
    }

    private let outputLock = NSLock()
    private func emit(_ value: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: JSONValue.bridgeSafe(value)) else { return }
        outputLock.lock(); defer { outputLock.unlock() }
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([10]))
    }

    private func startInputLoop() {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            var bytes = [UInt8](repeating: 0, count: 4096)
            var pending = Data()
            // Read incrementally so a client cannot allocate an unbounded request.
            while true {
                let count = Darwin.read(STDIN_FILENO, &bytes, bytes.count)
                if count < 0 && errno == EINTR { continue }
                if count <= 0 { break }
                pending.append(contentsOf: bytes.prefix(count))
                while let end = pending.firstIndex(of: 10) {
                    let line = pending[..<end]
                    if line.count <= 65536,
                       let message = try? JSONSerialization.jsonObject(with: line) as? [String: Any] {
                        DispatchQueue.main.async { self?.handle(message) }
                    }
                    pending.removeSubrange(...end)
                }
                if pending.count > 65536 { break }
            }
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }

    private func handle(_ message: [String: Any]) {
        let id = message["id"] as? Int ?? 0
        let method = message["method"] as? String ?? "GET"
        let path = message["path"] as? String ?? ""
        let body = message["body"] as? [String: Any] ?? [:]
        let allowedGET = ["/api/stats", "/api/settings", "/api/history?range=24h", "/api/history?range=10d"]
        let allowedPOST = ["/api/settings", "/api/ignore", "/api/kill", "/api/lowpower"]
        guard (method == "GET" && allowedGET.contains(path)) || (method == "POST" && allowedPOST.contains(path)) else {
            emit(["event": "response", "id": id, "path": path, "status": 400, "body": ["ok": false, "message": "Unsupported request"]]); return
        }
        if method == "POST" && ["/api/kill", "/api/lowpower"].contains(path) {
            let alert = NSAlert()
            alert.messageText = path == "/api/kill"
                ? L("Quit {name}?", ["name": body["name"] as? String ?? ""])
                : L("Low Power Mode")
            alert.informativeText = path == "/api/kill" ? L("It will be asked to close — save your work first.") : L("macOS may ask for your password.")
            alert.addButton(withTitle: L("OK")); alert.addButton(withTitle: L("Cancel"))
            NSApp.activate(ignoringOtherApps: true)
            guard alert.runModal() == .alertFirstButtonReturn else {
                emit(["event": "response", "id": id, "path": path, "status": 200, "body": ["ok": true, "cancelled": true]]); return
            }
        }
        backend.request(method: method, path: path, body: body) { [weak self] response in
            self?.emit(["event": "response", "id": id, "path": path, "status": response.status, "body": response.body])
            if method == "POST" { DispatchQueue.main.async { self?.pollStats() } }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        pollTimer?.invalidate()
        heatEvaluationWorkItem?.cancel()
        NotificationCenter.default.removeObserver(self)
        backend?.stop()
    }
}

@main enum BatteryHogService {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = GPUIService()
        app.delegate = delegate
        app.run()
        withExtendedLifetime(delegate) {}
    }
}
