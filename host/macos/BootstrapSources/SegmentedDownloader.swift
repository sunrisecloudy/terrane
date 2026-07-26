import Foundation

struct DownloadSegment: Equatable {
  let index: Int
  let start: Int64
  let end: Int64

  var length: Int64 { end - start + 1 }

  static func plan(size: Int64, connectionLimit: Int) -> [DownloadSegment] {
    guard size > 0 else { return [] }
    let count = max(1, min(connectionLimit, 8, Int(size)))
    let base = size / Int64(count)
    let remainder = size % Int64(count)
    var cursor: Int64 = 0
    return (0..<count).map { index in
      let length = base + (Int64(index) < remainder ? 1 : 0)
      let segment = DownloadSegment(index: index, start: cursor, end: cursor + length - 1)
      cursor += length
      return segment
    }
  }
}

struct TransferProgress: Equatable {
  let receivedBytes: Int64
  let totalBytes: Int64
  let bytesPerSecond: Double?
  let estimatedSecondsRemaining: TimeInterval?
  let elapsed: TimeInterval
  let activeConnections: Int
}

struct TransferRateEstimator {
  private var samples: [(date: Date, bytes: Int64)] = []
  private let window: TimeInterval

  init(window: TimeInterval = 5) {
    self.window = max(1, window)
  }

  mutating func record(totalBytes: Int64, at date: Date = Date()) {
    samples.append((date, totalBytes))
    let cutoff = date.addingTimeInterval(-window)
    while samples.count > 2, samples[1].date < cutoff {
      samples.removeFirst()
    }
  }

  func rate(at date: Date = Date()) -> Double? {
    guard let first = samples.first, let last = samples.last else { return nil }
    let duration = last.date.timeIntervalSince(first.date)
    guard duration >= 0.25, last.bytes >= first.bytes else { return nil }
    return Double(last.bytes - first.bytes) / duration
  }
}

protocol SegmentedDownloaderDelegate: AnyObject {
  func segmentedDownloader(_ downloader: SegmentedDownloader, didUpdate progress: TransferProgress)
  func segmentedDownloader(
    _ downloader: SegmentedDownloader, didRetry reason: String, attempt: Int, maximum: Int)
  func segmentedDownloader(
    _ downloader: SegmentedDownloader, didComplete result: Result<URL, Error>)
}

final class SegmentedDownloader: NSObject, URLSessionDataDelegate, URLSessionTaskDelegate {
  weak var delegate: SegmentedDownloaderDelegate?

  private final class TransferContext {
    let segment: DownloadSegment
    let generation: Int
    let file: FileHandle
    var received: Int64
    var responseError: Error?
    weak var task: URLSessionDataTask?

    init(segment: DownloadSegment, generation: Int, file: FileHandle, received: Int64) {
      self.segment = segment
      self.generation = generation
      self.file = file
      self.received = received
    }
  }

  private let url: URL
  private let expectedSize: Int64
  private let destination: URL
  private let partsDirectory: URL
  private let connectionLimit: Int
  private let stallTimeout: TimeInterval
  private let maximumRetries: Int
  private let fileManager: FileManager
  private let delegateQueue: OperationQueue
  private var session: URLSession!
  private var contexts: [Int: TransferContext] = [:]
  private var retryCounts: [Int: Int] = [:]
  private var segments: [DownloadSegment] = []
  private var completedSegments: Set<Int> = []
  private var receivedBytes: Int64 = 0
  private var startedAt = Date()
  private var lastProgressAt = Date()
  private var estimator = TransferRateEstimator()
  private var stallRetries = 0
  private var generation = 0
  private var usesRanges = false
  private var finished = false
  private var stallTimer: DispatchSourceTimer?

  init(
    url: URL,
    expectedSize: Int64,
    destination: URL,
    partsDirectory: URL,
    connectionLimit: Int = 8,
    stallTimeout: TimeInterval = 12,
    maximumRetries: Int = 3,
    protocolClasses: [AnyClass]? = nil,
    fileManager: FileManager = .default
  ) {
    self.url = url
    self.expectedSize = expectedSize
    self.destination = destination
    self.partsDirectory = partsDirectory
    self.connectionLimit = max(1, min(connectionLimit, 8))
    self.stallTimeout = max(2, stallTimeout)
    self.maximumRetries = max(0, maximumRetries)
    self.fileManager = fileManager
    delegateQueue = OperationQueue()
    delegateQueue.name = "com.terrane.bootstrap.downloader"
    delegateQueue.maxConcurrentOperationCount = 1
    super.init()

    let configuration = URLSessionConfiguration.ephemeral
    configuration.waitsForConnectivity = true
    configuration.timeoutIntervalForRequest = max(30, self.stallTimeout * 2)
    configuration.timeoutIntervalForResource = 60 * 60
    configuration.httpMaximumConnectionsPerHost = self.connectionLimit
    if let protocolClasses {
      configuration.protocolClasses = protocolClasses
    }
    session = URLSession(configuration: configuration, delegate: self, delegateQueue: delegateQueue)
  }

  func start() {
    delegateQueue.addOperation { [weak self] in
      guard let self, !self.finished else { return }
      do {
        try self.fileManager.createDirectory(
          at: self.partsDirectory, withIntermediateDirectories: true)
        self.startedAt = Date()
        self.lastProgressAt = self.startedAt
        self.probeRangeSupport()
      } catch {
        self.complete(.failure(error))
      }
    }
  }

  func cancel() {
    delegateQueue.addOperation { [weak self] in
      guard let self else { return }
      self.finished = true
      self.stopStallTimer()
      self.cancelActiveTransfers()
      self.session.invalidateAndCancel()
    }
  }

  private func probeRangeSupport() {
    var request = URLRequest(url: url)
    request.httpMethod = "HEAD"
    request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
    session.dataTask(with: request) { [weak self] _, response, _ in
      guard let self else { return }
      self.delegateQueue.addOperation {
        guard !self.finished else { return }
        let http = response as? HTTPURLResponse
        let acceptsRanges =
          http?.value(forHTTPHeaderField: "Accept-Ranges")?.lowercased() == "bytes"
        let length = http?.expectedContentLength ?? -1
        if http.map({ (200...299).contains($0.statusCode) }) == true,
          acceptsRanges, length == self.expectedSize, self.expectedSize >= 2
        {
          self.beginTransfers(useRanges: true)
        } else {
          self.beginTransfers(useRanges: false)
        }
      }
    }.resume()
  }

  private func beginTransfers(useRanges: Bool) {
    usesRanges = useRanges
    generation += 1
    completedSegments.removeAll()
    retryCounts.removeAll()
    stallRetries = 0
    if useRanges {
      segments = DownloadSegment.plan(size: expectedSize, connectionLimit: connectionLimit)
    } else {
      try? fileManager.removeItem(at: partsDirectory)
      try? fileManager.createDirectory(at: partsDirectory, withIntermediateDirectories: true)
      segments = [DownloadSegment(index: 0, start: 0, end: expectedSize - 1)]
    }
    receivedBytes = 0
    for segment in segments {
      let existing = validExistingBytes(for: segment)
      receivedBytes += existing
      if existing == segment.length {
        completedSegments.insert(segment.index)
      }
    }
    estimator = TransferRateEstimator()
    estimator.record(totalBytes: receivedBytes, at: Date())
    lastProgressAt = Date()
    emitProgress()
    startStallTimer()
    if completedSegments.count == segments.count {
      assemble()
      return
    }
    for segment in segments where !completedSegments.contains(segment.index) {
      startTransfer(for: segment)
    }
  }

  private func validExistingBytes(for segment: DownloadSegment) -> Int64 {
    let path = partURL(for: segment)
    guard
      let attributes = try? fileManager.attributesOfItem(atPath: path.path),
      let size = (attributes[.size] as? NSNumber)?.int64Value,
      size >= 0, size <= segment.length
    else {
      try? fileManager.removeItem(at: path)
      return 0
    }
    return size
  }

  private func startTransfer(for segment: DownloadSegment) {
    guard !finished else { return }
    let existing = validExistingBytes(for: segment)
    if existing == segment.length {
      completedSegments.insert(segment.index)
      finishIfReady()
      return
    }
    if !usesRanges, existing > 0 {
      try? fileManager.removeItem(at: partURL(for: segment))
    }
    let offset = usesRanges ? existing : 0
    let path = partURL(for: segment)
    if !fileManager.fileExists(atPath: path.path) {
      fileManager.createFile(atPath: path.path, contents: nil)
    }
    do {
      let handle = try FileHandle(forWritingTo: path)
      try handle.seekToEnd()
      var request = URLRequest(url: url)
      request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
      if usesRanges {
        request.setValue(
          "bytes=\(segment.start + offset)-\(segment.end)", forHTTPHeaderField: "Range")
      }
      let task = session.dataTask(with: request)
      let context = TransferContext(
        segment: segment, generation: generation, file: handle, received: offset)
      context.task = task
      contexts[task.taskIdentifier] = context
      task.resume()
    } catch {
      complete(.failure(error))
    }
  }

  func urlSession(
    _ session: URLSession,
    dataTask: URLSessionDataTask,
    didReceive response: URLResponse,
    completionHandler: @escaping (URLSession.ResponseDisposition) -> Void
  ) {
    guard let context = contexts[dataTask.taskIdentifier], context.generation == generation,
      let http = response as? HTTPURLResponse
    else {
      completionHandler(.cancel)
      return
    }
    if usesRanges {
      let expectedStart = context.segment.start + context.received
      let expectedRange =
        "bytes \(expectedStart)-\(context.segment.end)/\(expectedSize)"
      guard http.statusCode == 206,
        http.value(forHTTPHeaderField: "Content-Range")?.lowercased()
          == expectedRange.lowercased()
      else {
        context.responseError = BootstrapError.download(
          "release server did not honor byte ranges")
        completionHandler(.cancel)
        return
      }
    } else if !(200...299).contains(http.statusCode) {
      context.responseError = BootstrapError.download(
        "release server returned HTTP \(http.statusCode)")
      completionHandler(.cancel)
      return
    }
    completionHandler(.allow)
  }

  func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
    guard let context = contexts[dataTask.taskIdentifier], context.generation == generation,
      !finished
    else { return }
    guard context.received + Int64(data.count) <= context.segment.length else {
      context.responseError = BootstrapError.artifactSize(
        expected: context.segment.length, actual: context.received + Int64(data.count))
      dataTask.cancel()
      return
    }
    do {
      try context.file.write(contentsOf: data)
      context.received += Int64(data.count)
      receivedBytes += Int64(data.count)
      lastProgressAt = Date()
      estimator.record(totalBytes: receivedBytes, at: lastProgressAt)
      emitProgress()
    } catch {
      context.responseError = error
      dataTask.cancel()
    }
  }

  func urlSession(
    _ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?
  ) {
    guard let context = contexts.removeValue(forKey: task.taskIdentifier),
      context.generation == generation, !finished
    else { return }
    try? context.file.close()
    if let responseError = context.responseError {
      if usesRanges, let bootstrapError = responseError as? BootstrapError,
        case .download = bootstrapError
      {
        fallBackToSingleStream()
      } else {
        retryOrFail(segment: context.segment, error: responseError)
      }
      return
    }
    if let error {
      retryOrFail(
        segment: context.segment, error: BootstrapError.download(error.localizedDescription))
      return
    }
    guard validExistingBytes(for: context.segment) == context.segment.length else {
      retryOrFail(
        segment: context.segment,
        error: BootstrapError.download("a download segment ended before it was complete"))
      return
    }
    completedSegments.insert(context.segment.index)
    finishIfReady()
  }

  private func retryOrFail(segment: DownloadSegment, error: Error) {
    let attempt = (retryCounts[segment.index] ?? 0) + 1
    retryCounts[segment.index] = attempt
    guard attempt <= maximumRetries else {
      complete(.failure(error))
      return
    }
    delegate?.segmentedDownloader(
      self,
      didRetry: "Connection interrupted — resuming automatically",
      attempt: attempt,
      maximum: maximumRetries
    )
    if !usesRanges {
      receivedBytes = 0
      estimator = TransferRateEstimator()
      estimator.record(totalBytes: 0, at: Date())
    }
    startTransfer(for: segment)
  }

  private func fallBackToSingleStream() {
    cancelActiveTransfers()
    stopStallTimer()
    delegate?.segmentedDownloader(
      self,
      didRetry: "Parallel download is unavailable — using one connection",
      attempt: 1,
      maximum: 1
    )
    beginTransfers(useRanges: false)
  }

  private func startStallTimer() {
    stopStallTimer()
    let timer = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
    timer.schedule(deadline: .now() + 1, repeating: 1)
    timer.setEventHandler { [weak self] in
      guard let self else { return }
      self.delegateQueue.addOperation { self.checkForStall() }
    }
    stallTimer = timer
    timer.resume()
  }

  private func stopStallTimer() {
    stallTimer?.cancel()
    stallTimer = nil
  }

  private func checkForStall() {
    guard !finished, !contexts.isEmpty,
      Date().timeIntervalSince(lastProgressAt) >= stallTimeout
    else { return }
    stallRetries += 1
    guard stallRetries <= maximumRetries else {
      complete(
        .failure(
          BootstrapError.download(
            "download stalled after \(maximumRetries) automatic retries")))
      return
    }
    delegate?.segmentedDownloader(
      self,
      didRetry: "Download stalled — reconnecting automatically",
      attempt: stallRetries,
      maximum: maximumRetries
    )
    generation += 1
    cancelActiveTransfers()
    receivedBytes = segments.reduce(0) { $0 + validExistingBytes(for: $1) }
    completedSegments = Set(
      segments
        .filter { validExistingBytes(for: $0) == $0.length }
        .map(\.index)
    )
    estimator = TransferRateEstimator()
    estimator.record(totalBytes: receivedBytes, at: Date())
    lastProgressAt = Date()
    emitProgress()
    if completedSegments.count == segments.count {
      assemble()
      return
    }
    for segment in segments where validExistingBytes(for: segment) < segment.length {
      startTransfer(for: segment)
    }
  }

  private func finishIfReady() {
    if completedSegments.count == segments.count {
      assemble()
    }
  }

  private func assemble() {
    stopStallTimer()
    cancelActiveTransfers()
    let temporary = destination.appendingPathExtension("assembling")
    do {
      try? fileManager.removeItem(at: temporary)
      fileManager.createFile(atPath: temporary.path, contents: nil)
      let output = try FileHandle(forWritingTo: temporary)
      defer { try? output.close() }
      for segment in segments.sorted(by: { $0.index < $1.index }) {
        let input = try FileHandle(forReadingFrom: partURL(for: segment))
        defer { try? input.close() }
        while true {
          let data = try input.read(upToCount: 1024 * 1024) ?? Data()
          if data.isEmpty { break }
          try output.write(contentsOf: data)
        }
      }
      let size =
        (try fileManager.attributesOfItem(atPath: temporary.path)[.size] as? NSNumber)?
        .int64Value ?? -1
      guard size == expectedSize else {
        throw BootstrapError.artifactSize(expected: expectedSize, actual: size)
      }
      if fileManager.fileExists(atPath: destination.path) {
        try fileManager.removeItem(at: destination)
      }
      try fileManager.moveItem(at: temporary, to: destination)
      try? fileManager.removeItem(at: partsDirectory)
      complete(.success(destination))
    } catch {
      try? fileManager.removeItem(at: temporary)
      complete(.failure(error))
    }
  }

  private func emitProgress() {
    let now = Date()
    let rate = estimator.rate(at: now)
    let remaining =
      rate.flatMap { $0 > 0 ? Double(max(0, expectedSize - receivedBytes)) / $0 : nil }
    delegate?.segmentedDownloader(
      self,
      didUpdate: TransferProgress(
        receivedBytes: receivedBytes,
        totalBytes: expectedSize,
        bytesPerSecond: rate,
        estimatedSecondsRemaining: remaining,
        elapsed: now.timeIntervalSince(startedAt),
        activeConnections: contexts.count
      ))
  }

  private func cancelActiveTransfers() {
    let active = contexts
    contexts.removeAll()
    for context in active.values {
      try? context.file.close()
      context.task?.cancel()
    }
  }

  private func partURL(for segment: DownloadSegment) -> URL {
    partsDirectory.appendingPathComponent(String(format: "part-%03d", segment.index))
  }

  private func complete(_ result: Result<URL, Error>) {
    guard !finished else { return }
    finished = true
    stopStallTimer()
    cancelActiveTransfers()
    session.finishTasksAndInvalidate()
    delegate?.segmentedDownloader(self, didComplete: result)
  }
}
