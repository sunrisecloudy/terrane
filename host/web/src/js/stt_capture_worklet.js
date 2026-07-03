// AudioWorklet processor: float32 mic frames → Int16 PCM for the STT WebSocket.
class SttCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    var input = inputs[0];
    if (!input || !input[0] || !input[0].length) return true;
    var channel = input[0];
    var int16 = new Int16Array(channel.length);
    for (var i = 0; i < channel.length; i++) {
      var sample = channel[i];
      if (sample > 1) sample = 1;
      if (sample < -1) sample = -1;
      int16[i] = sample < 0 ? sample * 0x8000 : sample * 0x7fff;
    }
    this.port.postMessage(int16.buffer, [int16.buffer]);
    return true;
  }
}

registerProcessor("stt-capture-processor", SttCaptureProcessor);