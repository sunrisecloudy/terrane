import Foundation

final class PlatformNetwork {
    private let core = ForgeCoreBridge()

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

        let path = Self.path(for: url)
        let rule = request.context.networkPolicy.first(where: { $0.matchesTarget(origin: origin, method: method, path: path) })
        if let preflightFailure = validateNetworkRequest(
            request: request,
            urlText: urlText,
            method: method,
            headers: headers,
            bodyData: bodyData,
            rule: rule
        ) {
            return preflightFailure
        }
        guard let rule else {
            return .failure(
                id: request.id,
                code: "network_policy_denied",
                message: "network.request is not allowed by manifest.networkPolicy",
                details: Self.networkPolicyDeniedDetails(origin: origin, method: method)
            )
        }
        let requestedTimeout = Self.requestedTimeoutMs(from: request.params)
        if case let .invalid(value) = requestedTimeout {
            return .failure(
                id: request.id,
                code: "invalid_request",
                message: "network.request timeoutMs must be a positive integer",
                details: ["timeoutMs": value]
            )
        }
        let effectiveTimeoutMs = Self.effectiveTimeoutMs(rule: rule, requested: requestedTimeout.value)

        var urlRequest = URLRequest(url: url)
        urlRequest.httpMethod = method
        urlRequest.timeoutInterval = TimeInterval(effectiveTimeoutMs) / 1000.0
        urlRequest.httpShouldHandleCookies = false
        urlRequest.httpBody = bodyData
        for (name, value) in headers.values {
            urlRequest.setValue(value, forHTTPHeaderField: name)
        }

        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieAcceptPolicy = .never
        configuration.httpShouldSetCookies = false
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.timeoutIntervalForRequest = urlRequest.timeoutInterval
        configuration.timeoutIntervalForResource = urlRequest.timeoutInterval

        let redirectGuard = NetworkRedirectGuard(
            policy: request.context.networkPolicy,
            denyPrivateNetwork: request.context.denyPrivateNetwork,
            method: method,
            sandboxContext: request.context,
            network: self
        )
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
            return Self.timeoutFailure(id: request.id, timeoutMs: effectiveTimeoutMs)
        }

        if let redirectFailure = redirectGuard.deniedFailure {
            return .failure(
                id: request.id,
                code: "network_policy_denied",
                message: redirectFailure.message,
                details: redirectFailure.details
            )
        }
        let result = resultBox.value()
        if let responseError = result.error {
            let error = responseError as NSError
            if error.domain == NSURLErrorDomain && error.code == NSURLErrorTimedOut {
                return Self.timeoutFailure(id: request.id, timeoutMs: effectiveTimeoutMs)
            }
            return .failure(id: request.id, code: "network_error", message: responseError.localizedDescription)
        }
        guard let httpResponse = result.response as? HTTPURLResponse else {
            return .failure(id: request.id, code: "network_error", message: "network.request did not return an HTTP response")
        }
        let data = result.data ?? Data()
        if let responseFailure = validateNetworkResponse(
            request: request,
            urlText: urlText,
            method: method,
            headers: headers,
            responseBytes: data.count,
            rule: rule
        ) {
            return responseFailure
        }

        return .success(id: request.id, result: [
            "status": httpResponse.statusCode,
            "headers": Self.responseHeaders(from: httpResponse),
            "bodyText": String(data: data, encoding: .utf8) ?? ""
        ])
    }

    private func validateNetworkRequest(
        request: BridgeRequest,
        urlText: String,
        method: String,
        headers: NetworkHeaders,
        bodyData: Data?,
        rule: NetworkPolicyRule?
    ) -> BridgeResponse? {
        if let decision = coreNetworkDecision(
            request: request,
            urlText: urlText,
            method: method,
            headers: headers,
            bodyData: bodyData,
            responseBytes: nil,
            redirectURL: nil
        ) {
            return networkDecisionFailure(id: request.id, decision: decision)
        }
        guard let rule else { return nil }
        if let headerFailure = Self.headerPolicyFailure(id: request.id, headers: headers, rule: rule) {
            return headerFailure
        }
        if let credentials = request.params["credentials"], !(credentials is NSNull) {
            return .failure(
                id: request.id,
                code: "network_policy_denied",
                message: "network.request credentials are not allowed",
                details: ["credentials": credentials]
            )
        }
        if let bodyData, bodyData.count > rule.maxRequestBytes {
            return .failure(
                id: request.id,
                code: "network_policy_denied",
                message: "network.request body exceeds manifest.networkPolicy maxRequestBytes",
                details: Self.maxRequestBytesDetails(limit: rule.maxRequestBytes, bytes: bodyData.count)
            )
        }
        return nil
    }

    private func validateNetworkResponse(
        request: BridgeRequest,
        urlText: String,
        method: String,
        headers: NetworkHeaders,
        responseBytes: Int,
        rule: NetworkPolicyRule
    ) -> BridgeResponse? {
        if let decision = coreNetworkDecision(
            request: request,
            urlText: urlText,
            method: method,
            headers: headers,
            bodyData: nil,
            responseBytes: responseBytes,
            redirectURL: nil
        ) {
            return networkDecisionFailure(id: request.id, decision: decision)
        }
        let maxResponseBytes = Self.effectiveMaxResponseBytes(rule: rule, resourceBudget: request.context.resourceBudget)
        if responseBytes > maxResponseBytes {
            return .failure(
                id: request.id,
                code: "network_policy_denied",
                message: "network.response exceeds manifest.networkPolicy maxResponseBytes",
                details: Self.maxResponseBytesDetails(limit: maxResponseBytes, bytes: responseBytes)
            )
        }
        return nil
    }

    func validateRedirectTarget(
        request: BridgeRequest,
        redirectURL: String,
        method: String
    ) -> NetworkPolicyFailure? {
        if core.isAvailable {
            if let decision = coreNetworkDecision(
                request: request,
                urlText: redirectURL,
                method: method,
                headers: NetworkHeaders(),
                bodyData: nil,
                responseBytes: nil,
                redirectURL: redirectURL
            ) {
                return NetworkPolicyFailure(
                    message: decision["message"] as? String ?? "network.response redirect is outside manifest.networkPolicy",
                    details: decision["details"] as? [String: Any] ?? [:]
                )
            }
            return nil
        }
        guard let url = URL(string: redirectURL),
              let origin = Self.origin(for: url)
        else {
            return NetworkPolicyFailure(message: "network.response redirect is outside manifest.networkPolicy", details: [:])
        }
        if request.context.denyPrivateNetwork && Self.isPrivateNetworkHost(url.host) {
            return NetworkPolicyFailure(
                message: "network.response redirect targets private network",
                details: Self.privateNetworkDeniedDetails(origin: origin, host: url.host)
            )
        }
        guard request.context.networkPolicy.contains(where: {
            $0.matchesTarget(origin: origin, method: method, path: Self.path(for: url))
        }) else {
            return NetworkPolicyFailure(
                message: "network.response redirect is outside manifest.networkPolicy",
                details: Self.networkPolicyDeniedDetails(origin: origin, method: method)
            )
        }
        return nil
    }

    private func coreNetworkDecision(
        request: BridgeRequest,
        urlText: String,
        method: String,
        headers: NetworkHeaders,
        bodyData: Data?,
        responseBytes: Int?,
        redirectURL: String?
    ) -> [String: Any]? {
        guard core.isAvailable else { return nil }
        var netRequest: [String: Any] = [
            "url": urlText,
            "method": method,
        ]
        if !headers.values.isEmpty {
            netRequest["headers"] = headers.values
        }
        if let bodyData {
            netRequest["body_bytes"] = bodyData.count
        }
        if let credentials = request.params["credentials"] {
            netRequest["credentials"] = credentials
        }
        if let timeoutMs = Self.requestedTimeoutMs(from: request.params).value {
            netRequest["timeout_ms"] = timeoutMs
        }
        if let responseBytes {
            netRequest["response_bytes"] = responseBytes
        }
        if let redirectURL {
            netRequest["redirect_url"] = redirectURL
        }
        guard let decision = core.bridgeCommandDictionary(
            name: "bridge.validate_network_request",
            payload: [
                "network_policy": request.context.networkPolicyPayload,
                "request": netRequest,
                "resource_budget": request.context.resourceBudgetPayload,
            ],
            requestId: request.id ?? "macos-network"
        ) else {
            return nil
        }
        if decision["allowed"] as? Bool == true {
            return nil
        }
        return decision
    }

    private func networkDecisionFailure(id: String?, decision: [String: Any]) -> BridgeResponse {
        .failure(
            id: id,
            code: decision["error_code"] as? String ?? "network_policy_denied",
            message: decision["message"] as? String ?? "network.request denied",
            details: decision["details"] as? [String: Any] ?? [:]
        )
    }

    static func origin(for url: URL) -> String? {
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

    static func path(for url: URL) -> String {
        url.path.isEmpty ? "/" : url.path
    }

    private static func headers(from value: Any?) -> NetworkHeaders? {
        guard let value else {
            return NetworkHeaders()
        }
        if value is NSNull {
            return NetworkHeaders()
        }
        guard let raw = value as? [String: Any] else {
            return nil
        }
        var headers: [String: String] = [:]
        var normalizedNames: [String] = []
        var originalNames: [String: String] = [:]
        for (name, headerValue) in raw {
            guard let text = headerValue as? String else {
                return nil
            }
            let normalized = name.lowercased()
            headers[normalized] = text
            normalizedNames.append(normalized)
            originalNames[normalized] = name
        }
        return NetworkHeaders(values: headers, normalizedNames: normalizedNames, originalNames: originalNames)
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

    private static func requestedTimeoutMs(from params: [String: Any]) -> RequestedTimeout {
        guard let value = params["timeoutMs"] else {
            return .absent
        }
        if value is Bool {
            return .invalid(value)
        }
        if let intValue = value as? Int {
            return intValue > 0 ? .valid(intValue) : .invalid(value)
        }
        if let doubleValue = value as? Double {
            return doubleValue.isFinite && doubleValue > 0 && doubleValue <= Double(Int.max) && doubleValue.rounded(.towardZero) == doubleValue
                ? .valid(Int(doubleValue))
                : .invalid(value)
        }
        if let number = value as? NSNumber {
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return .invalid(value)
            }
            let doubleValue = number.doubleValue
            return doubleValue.isFinite && doubleValue > 0 && doubleValue <= Double(Int.max) && doubleValue.rounded(.towardZero) == doubleValue
                ? .valid(Int(doubleValue))
                : .invalid(value)
        }
        return .invalid(value)
    }

    private static func effectiveTimeoutMs(rule: NetworkPolicyRule, requested: Int?) -> Int {
        requested.map { min(rule.timeoutMs, $0) } ?? rule.timeoutMs
    }

    private static func timeoutFailure(id: String?, timeoutMs: Int) -> BridgeResponse {
        .failure(id: id, code: "timeout", message: "network.request timed out", details: ["timeoutMs": timeoutMs])
    }

    private static func headerPolicyFailure(id: String?, headers: NetworkHeaders, rule: NetworkPolicyRule) -> BridgeResponse? {
        guard let violation = rule.headerViolation(in: headers.normalizedNames) else {
            return nil
        }
        let header = headers.originalName(for: violation.header)
        var details: [String: Any] = ["header": header]
        if !violation.credential {
            details["allowedHeaders"] = Array(rule.allowedHeaders).sorted()
        }
        return .failure(
            id: id,
            code: "network_policy_denied",
            message: violation.credential
                ? "network.request credential headers are not allowed"
                : "network.request header is outside manifest.networkPolicy",
            details: details
        )
    }

    static func networkPolicyDeniedDetails(origin: String, method: String) -> [String: Any] {
        ["origin": origin, "method": method]
    }

    static func privateNetworkDeniedDetails(origin: String, host: String?) -> [String: Any] {
        ["origin": origin, "host": normalizedNetworkHost(host)]
    }

    static func maxRequestBytesDetails(limit: Int, bytes: Int) -> [String: Any] {
        ["maxRequestBytes": limit, "bytes": bytes]
    }

    static func maxResponseBytesDetails(limit: Int, bytes: Int) -> [String: Any] {
        ["maxResponseBytes": limit, "bytes": bytes]
    }

    static func effectiveMaxResponseBytes(rule: NetworkPolicyRule, resourceBudget: [String: Int]) -> Int {
        guard let budgetLimit = resourceBudget["maxNetworkResponseBytes"] else {
            return rule.maxResponseBytes
        }
        return min(rule.maxResponseBytes, budgetLimit)
    }

    static func isPrivateNetworkHost(_ rawHost: String?) -> Bool {
        let host = normalizedNetworkHost(rawHost)
        if host == "localhost" || host.hasSuffix(".localhost") {
            return true
        }
        if let octets = ipv4Octets(host) {
            return privateIpv4Octets(octets)
        }
        if host == "::1" {
            return true
        }
        if host.hasPrefix("fc") || host.hasPrefix("fd") {
            return true
        }
        if host.hasPrefix("fe8") || host.hasPrefix("fe9") || host.hasPrefix("fea") || host.hasPrefix("feb") {
            return true
        }
        if host.hasPrefix("::ffff:") {
            return privateIpv4MappedHost(String(host.dropFirst("::ffff:".count)))
        }
        return false
    }

    static func normalizedNetworkHost(_ rawHost: String?) -> String {
        var host = (rawHost ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if host.hasPrefix("[") && host.hasSuffix("]") {
            host = String(host.dropFirst().dropLast())
        }
        if let zone = host.firstIndex(of: "%") {
            host = String(host[..<zone])
        }
        return host
    }

    private static func ipv4Octets(_ host: String) -> [UInt8]? {
        let parts = host.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        var octets: [UInt8] = []
        for part in parts {
            guard let value = UInt8(String(part)) else { return nil }
            octets.append(value)
        }
        return octets
    }

    private static func privateIpv4MappedHost(_ tail: String) -> Bool {
        if let octets = ipv4Octets(tail) {
            return privateIpv4Octets(octets)
        }
        let parts = tail.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 2,
              let high = UInt16(String(parts[0]), radix: 16),
              let low = UInt16(String(parts[1]), radix: 16)
        else {
            return false
        }
        return privateIpv4Octets([
            UInt8((high >> 8) & 0x00ff),
            UInt8(high & 0x00ff),
            UInt8((low >> 8) & 0x00ff),
            UInt8(low & 0x00ff)
        ])
    }

    private static func privateIpv4Octets(_ octets: [UInt8]) -> Bool {
        guard octets.count == 4 else { return false }
        let first = octets[0]
        let second = octets[1]
        return first == 0 ||
            first == 10 ||
            first == 127 ||
            (first == 100 && second >= 64 && second <= 127) ||
            (first == 169 && second == 254) ||
            (first == 172 && second >= 16 && second <= 31) ||
            (first == 192 && second == 168)
    }
}

struct NetworkPolicyRule {
    let origin: String
    let methods: Set<String>
    let pathPrefix: String?
    let allowedHeaders: Set<String>
    let maxRequestBytes: Int
    let maxResponseBytes: Int
    let timeoutMs: Int

    func matchesTarget(origin: String, method: String, path: String) -> Bool {
        guard self.origin == origin, methods.contains(method) else {
            return false
        }
        if let pathPrefix, !path.hasPrefix(pathPrefix) {
            return false
        }
        return true
    }

    func headerViolation(in headers: [String]) -> NetworkPolicyHeaderViolation? {
        for header in headers {
            let normalized = header.lowercased()
            if normalized == "cookie" || normalized == "set-cookie" {
                return NetworkPolicyHeaderViolation(header: normalized, credential: true)
            }
        }
        for header in headers {
            let normalized = header.lowercased()
            if !allowedHeaders.contains(normalized) {
                return NetworkPolicyHeaderViolation(header: normalized, credential: false)
            }
        }
        return nil
    }

    func allows(origin: String, method: String, path: String, headers: [String]) -> Bool {
        guard matchesTarget(origin: origin, method: method, path: path) else {
            return false
        }
        return headerViolation(in: headers) == nil
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
                pathPrefix: raw["pathPrefix"] as? String,
                allowedHeaders: allowedHeaders,
                maxRequestBytes: raw["maxRequestBytes"] as? Int ?? 0,
                maxResponseBytes: raw["maxResponseBytes"] as? Int ?? 0,
                timeoutMs: raw["timeoutMs"] as? Int ?? 10_000
            )
        }
    }
}

struct NetworkPolicyHeaderViolation {
    let header: String
    let credential: Bool
}

struct NetworkPolicyFailure {
    let message: String
    let details: [String: Any]
}

private struct NetworkHeaders {
    let values: [String: String]
    let normalizedNames: [String]
    let originalNames: [String: String]

    init(values: [String: String] = [:], normalizedNames: [String] = [], originalNames: [String: String] = [:]) {
        self.values = values
        self.normalizedNames = normalizedNames
        self.originalNames = originalNames
    }

    func originalName(for normalized: String) -> String {
        originalNames[normalized] ?? normalized
    }
}

private final class NetworkRedirectGuard: NSObject, URLSessionTaskDelegate {
    private let policy: [NetworkPolicyRule]
    private let denyPrivateNetwork: Bool
    private let method: String
    private let sandboxContext: AppSandboxContext
    private weak var network: PlatformNetwork?
    private let state = NetworkRedirectState()

    var deniedFailure: NetworkPolicyFailure? {
        state.failure
    }

    init(
        policy: [NetworkPolicyRule],
        denyPrivateNetwork: Bool,
        method: String,
        sandboxContext: AppSandboxContext,
        network: PlatformNetwork
    ) {
        self.policy = policy
        self.denyPrivateNetwork = denyPrivateNetwork
        self.method = method
        self.sandboxContext = sandboxContext
        self.network = network
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard let url = request.url,
              PlatformNetwork.origin(for: url) != nil
        else {
            state.markDenied(NetworkPolicyFailure(message: "network.response redirect is outside manifest.networkPolicy", details: [:]))
            completionHandler(nil)
            return
        }
        if let network,
           let failure = network.validateRedirectTarget(
               request: BridgeRequest(
                   id: nil,
                   method: "network.request",
                   params: ["url": url.absoluteString],
                   context: sandboxContext
               ),
               redirectURL: url.absoluteString,
               method: method
           ) {
            state.markDenied(failure)
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

private enum RequestedTimeout {
    case absent
    case valid(Int)
    case invalid(Any)

    var value: Int? {
        if case let .valid(value) = self {
            return value
        }
        return nil
    }
}

private final class NetworkRedirectState: @unchecked Sendable {
    private let lock = NSLock()
    private var value: NetworkPolicyFailure?

    var failure: NetworkPolicyFailure? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func markDenied(_ failure: NetworkPolicyFailure) {
        lock.lock()
        value = failure
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
