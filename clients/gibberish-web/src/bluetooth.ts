//! Web Bluetooth API Transport for Project Gibberish (R20).

export const GIBBERISH_SERVICE_UUID = '0000ffe0-0000-1000-8000-00805f9b34fb';
export const GIBBERISH_CHAR_TX_UUID = '0000ffe1-0000-1000-8000-00805f9b34fb';
export const GIBBERISH_CHAR_RX_UUID = '0000ffe2-0000-1000-8000-00805f9b34fb';

export interface BleConnection {
  device: BluetoothDevice;
  server: BluetoothRemoteGATTServer;
  txChar: BluetoothRemoteGATTCharacteristic;
  rxChar: BluetoothRemoteGATTCharacteristic;
}

export async function connectBleDongle(
  onPacketReceived: (data: Uint8Array) => void
): Promise<BleConnection> {
  if (!navigator.bluetooth) {
    throw new Error('Web Bluetooth is not supported in this browser.');
  }

  const device = await navigator.bluetooth.requestDevice({
    filters: [{ namePrefix: 'Gibberish' }],
    optionalServices: [GIBBERISH_SERVICE_UUID],
  });

  const server = await device.gatt?.connect();
  if (!server) {
    throw new Error('Failed to connect to GATT server.');
  }

  const service = await server.getPrimaryService(GIBBERISH_SERVICE_UUID);
  const txChar = await service.getCharacteristic(GIBBERISH_CHAR_TX_UUID);
  const rxChar = await service.getCharacteristic(GIBBERISH_CHAR_RX_UUID);

  await rxChar.startNotifications();
  rxChar.addEventListener('characteristicvaluechanged', (event: any) => {
    const value = event.target.value as DataView;
    const bytes = new Uint8Array(value.buffer);
    onPacketReceived(bytes);
  });

  return { device, server, txChar, rxChar };
}

export async function sendBlePacket(
  connection: BleConnection,
  data: Uint8Array
): Promise<void> {
  await connection.txChar.writeValueWithoutResponse(data);
}
