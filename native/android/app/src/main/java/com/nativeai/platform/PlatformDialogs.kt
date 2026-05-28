package com.nativeai.platform

class PlatformDialogs {
    fun openFile(request: BridgeRequest): String = unsupported(request, "dialog.openFile")

    fun saveFile(request: BridgeRequest): String = unsupported(request, "dialog.saveFile")

    private fun unsupported(request: BridgeRequest, method: String): String = BridgeResponse.failure(
        request.id,
        "platform_unsupported",
        "$method is not implemented on Android yet",
    ).toString()
}
