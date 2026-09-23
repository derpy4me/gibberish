//! Web Serial API Transport for Project Gibberish (R20).

export interface SerialConnection {
  port: SerialPort;
  reader: ReadableStreamDefaultReader<Uint8Array>;
  writer: WritableStreamDefaultWriter<Uint8Array>;
}

export async function connectSerialDongle(
  onPacketReceived: (data: Uint8Array) => void
): Promise<SerialConnection> {
  if (!('serial' in navigator)) {
    throw new Error('Web Serial is not supported in this browser.');
  }

  const port = await (navigator as any).serial.requestPort();
  await port.open({ baudRate: 115200 });

  const reader = port.readable.getReader();
  const writer = port.writable.getWriter();

  // Background read loop
  (async () => {
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        if (value) {
          onPacketReceived(value);
        }
      }
    } catch (e) {
      console.error('Serial read loop error:', e);
    }
  })();

  return { port, reader, writer };
}

export async function sendSerialPacket(
  connection: SerialConnection,
  data: Uint8Array
): Promise<void> {
  await connection.writer.write(data);
}
