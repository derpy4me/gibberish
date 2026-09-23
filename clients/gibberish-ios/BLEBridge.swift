//
//  BLEBridge.swift
//  Gibberish
//
//  Lightweight iOS CoreBluetooth bridge for Project Gibberish (R21).
//

import Foundation
import CoreBluetooth

public protocol GibberishBridgeDelegate: AnyObject {
    func didDiscoverDongle(name: String)
    func didConnectDongle()
    func didDisconnectDongle()
    func didReceiveMeshPacket(data: Data)
}

public class GibberishBLEBridge: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    public static let serviceUUID = CBUUID(string: "0000FFE0-0000-1000-8000-00805F9B34FB")
    public static let txCharUUID  = CBUUID(string: "0000FFE1-0000-1000-8000-00805F9B34FB")
    public static let rxCharUUID  = CBUUID(string: "0000FFE2-0000-1000-8000-00805F9B34FB")

    private var centralManager: CBCentralManager!
    private var peripheral: CBPeripheral?
    private var txCharacteristic: CBCharacteristic?
    private var rxCharacteristic: CBCharacteristic?

    public weak var delegate: GibberishBridgeDelegate?

    public override init() {
        super.init()
        centralManager = CBCentralManager(delegate: self, queue: nil)
    }

    public func startScanning() {
        if centralManager.state == .poweredOn {
            centralManager.scanForPeripherals(withServices: [GibberishBLEBridge.serviceUUID], options: nil)
        }
    }

    public func sendPacket(data: Data) {
        guard let peripheral = peripheral, let tx = txCharacteristic else { return }
        peripheral.writeValue(data, for: tx, type: .withoutResponse)
    }

    // MARK: - CBCentralManagerDelegate

    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        if central.state == .poweredOn {
            startScanning()
        }
    }

    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String : Any], rssi RSSI: NSNumber) {
        self.peripheral = peripheral
        delegate?.didDiscoverDongle(name: peripheral.name ?? "Gibberish Node")
        central.stopScan()
        central.connect(peripheral, options: nil)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.delegate = self
        peripheral.discoverServices([GibberishBLEBridge.serviceUUID])
        delegate?.didConnectDongle()
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        self.peripheral = nil
        self.txCharacteristic = nil
        self.rxCharacteristic = nil
        delegate?.didDisconnectDongle()
    }

    // MARK: - CBPeripheralDelegate

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let services = peripheral.services else { return }
        for service in services where service.uuid == GibberishBLEBridge.serviceUUID {
            peripheral.discoverCharacteristics([GibberishBLEBridge.txCharUUID, GibberishBLEBridge.rxCharUUID], for: service)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard let characteristics = service.characteristics else { return }
        for char in characteristics {
            if char.uuid == GibberishBLEBridge.txCharUUID {
                self.txCharacteristic = char
            } else if char.uuid == GibberishBLEBridge.rxCharUUID {
                self.rxCharacteristic = char
                peripheral.setNotifyValue(true, for: char)
            }
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == GibberishBLEBridge.rxCharUUID, let data = characteristic.value else { return }
        delegate?.didReceiveMeshPacket(data: data)
    }
}
