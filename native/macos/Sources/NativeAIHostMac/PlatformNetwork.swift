import Foundation

final class PlatformNetwork {
    func request(_ request: BridgeRequest) -> BridgeResponse {
        guard let urlText = request.params["url"] as? String,
              let url = URL(string: urlText),
              let origin = Self.origin(for: url)
        else {
            return .failure(id: request.id, code: "invalid_request", message: "network.request requires an absolute url")
        }

        let method = (request.params["method"] as? String ?? "GET").uppercased()
        guard let headers = Self.headers(from: request.params["headers"]) else {
            return .failure(id: request.id, code: "invalid_request", message: "network.request headers must be strings")
        }

        let bodyResult = Self.bodyData(from: request.params["body"])
        guard case let .valid(bodyData) = bodyResult else {
            return .failure(id: request.id, code: "invalid_request", message: "network.request body must be a string or null")
        }

        guard let rule = request.context.networkPolicy.first(where: { $0.allows(origin: origin, method: method, headers: Array(headers.keys)) }) else {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request is not allowed by manifest.networkPolicy")
        }
        if let bodyData, bodyData.count > rule.maxRequestBytes {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request body exceeds manifest.networkPolicy maxRequestBytes")
        }

        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = method
        urlRequest.timeoutInterval = TimeInterval(rule.timeoutMs) / 1000.0
        urlRequest.httpShouldHandleCookies = false
        urlRequest.httpBody = bodyData
        for (name, value) in headers {
            urlRequest.setValue(value, forHTTPHeaderField: name)
        }

        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieAcceptPolicy = .never
        configuration.httpShouldSetCookies = false
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.timeoutIntervalForRequest = urlRequest.timeoutInterval
        configuration.timeoutIntervalForResource = urlRequest.timeoutInterval

        let redirectGuard = NetworkRedirectGuard(policy: request.context.networkPolicy, method: method, headers: Array(headers.keys))
        let session = URLSession(configuration: configuration, delegate: redirectGuard, delegateQueue: nil)
        defer {
            session.invalidateAndCancel()
        }

        let semaphore = DispatchSemaphore(value: 0)
        let resultBox = NetworkResultBox()
        session.dataTask(with: urlRequest) { data, urlResponse, error in
            resultBox.set(data: data, response: urlResponse, error: error)
            semaphore.signal()
        }.resume()
        if semaphore.wait(timeout: .now() + urlRequest.timeoutInterval + 1.0) == .timedOut {
            return .failure(id: request.id, code: "timeout", message: "network.request timed out")
        }

        if redirectGuard.deniedRedirect {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.request redirect is not allowed by manifest.networkPolicy")
        }
        let result = resultBox.value()
        if let responseError = result.error {
            return .failure(id: request.id, code: "network_error", message: responseError.localizedDescription)
        }
        guard let httpResponse = result.response as? HTTPURLResponse else {
            return .failure(id: request.id, code: "network_error", message: "network.request did not return an HTTP response")
        }
        let data = result.data ?? Data()
        if data.count > rule.maxResponseBytes {
            return .failure(id: request.id, code: "network_policy_denied", message: "network.response exceeds manifest.networkPolicy maxResponseBytes")
        }

        return .success(id: request.id, result: [
            "status": httpResponse.statusCode,
            "headers": Self.responseHeaders(from: httpResponse),
            "bodyText": String(data: data, encoding: .utf8) ?? ""
        ])
    }

    fileprivate static func origin(for url: URL) -> String? {
        guard let scheme = url.scheme?.lowercased(),
              let host = url.host?.lowercased(),
              scheme == "http" || scheme == "https"
        else {
            return nil
        }
        if let port = url.port,
           !(scheme == "http" && port == 80),
           !(scheme == "https" && port == 443) {
            return "\(scheme)://\(host):\(port)"
        }
        return "\(scheme)://\(host)"
    }

    private static func headers(from value: Any?) -> [String: String]? {
        guard let value else {
            return [:]
        }
        if value is NSNull {
            return [:]
        }
        guard let raw = value as? [String: Any] else {
            return nil
        }
        var headers: [String: String] = [:]
        for (name, headerValue) in raw {
            guard let text = headerValue as? String else {
                return nil
            }
            headers[name.lowercased()] = text
        }
        return headers
    }

    private static func bodyData(from value: Any?) -> NetworkBody {
        guard let value else {
            return .valid(nil)
        }
        if value is NSNull {
            return .valid(nil)
        }
        guard let text = value as? String else {
            return .invalid
        }
        return .valid(Data(text.utf8))
    }

    private static func responseHeaders(from response: HTTPURLResponse) -> [String: String] {
        var headers: [String: String] = [:]
        for (name, value) in response.allHeaderFields {
            guard let name = name as? String else { continue }
            headers[name.lowercased()] = String(describing: value)
        }
        return headers
    }
}

struct NetworkPolicyRule {
    let origin: String
    let methods: Set<String>
    let allowedHeaders: Set<String>
    let maxRequestBytes: Int
    let maxResponseBytes: Int
    let timeoutMs: Int

    func allows(origin: String, method: String, headers: [String]) -> Bool {
        guard self.origin == origin, methods.contains(method) else {
            return false
        }
        for header in headers {
            let normalized = header.lowercased()
            if normalized == "cookie" || normalized == "set-cookie" {
                return false
            }
            if !allowedHeaders.contains(normalized) {
                return false
            }
        }
        return true
    }

    static func fromManifest(_ manifest: [String: Any]) -> [NetworkPolicyRule] {
        guard let policy = manifest["networkPolicy"] as? [String: Any],
              let allow = policy["allow"] as? [[String: Any]]
        else {
            return []
        }
        return allow.compactMap { raw in
            guard let origin = raw["origin"] as? String else { return nil }
            let methods = Set((raw["methods"] as? [String] ?? []).map { $0.uppercased() })
            let allowedHeaders = Set((raw["allowedHeaders"] as? [String] ?? []).map { $0.lowercased() })
            return NetworkPolicyRule(
                origin: origin,
                methods: methods,
                allowedHeaders: allowedHeaders,
                maxRequestBytes: raw["maxRequestBytes"] as? Int ?? 0,
                maxResponseBytes: raw["maxResponseBytes"] as? Int ?? 0,
                timeoutMs: raw["timeoutMs"] as? Int ?? 10_000
            )
        }
    }
}

private final class NetworkRedirectGuard: NSObject, URLSessionTaskDelegate {
    private let policy: [NetworkPolicyRule]
    private let method: String
    private let headers: [String]
    private let state = NetworkRedirectState()

    var deniedRedirect: Bool {
        state.denied
    }

    init(policy: [NetworkPolicyRule], method: String, headers: [String]) {
        self.policy = policy
        self.method = method
        self.headers = headers
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard let url = request.url,
              let origin = PlatformNetwork.origin(for: url),
              policy.contains(where: { $0.allows(origin: origin, method: method, headers: headers) })
        else {
            state.markDenied()
            completionHandler(nil)
            return
        }
        completionHandler(request)
    }
}

private enum NetworkBody {
    case valid(Data?)
    case invalid
}

private final class NetworkRedirectState: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    var denied: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func markDenied() {
        lock.lock()
        value = true
        lock.unlock()
    }
}

private final class NetworkResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var storedData: Data?
    private var storedResponse: URLResponse?
    private var storedError: Error?

    func set(data: Data?, response: URLResponse?, error: Error?) {
        lock.lock()
        storedData = data
        storedResponse = response
        storedError = error
        lock.unlock()
    }

    func value() -> (data: Data?, response: URLResponse?, error: Error?) {
        lock.lock()
        defer { lock.unlock() }
        return (storedData, storedResponse, storedError)
    }
}
