import React, { useState, useEffect } from 'react';
import { Shield, Radio, Clipboard, HardDrive, Key, QrCode, RefreshCw } from 'lucide-react';

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<'clipboard' | 'chat' | 'vault' | 'keyring'>('clipboard');
  const [autoSync, setAutoSync] = useState(false);
  const [clipboardText, setClipboardText] = useState('Secret decentralized clipboard payload ready.');
  const [storageMode, setStorageMode] = useState<'RAM_ONLY' | 'SD_ACTIVE'>('RAM_ONLY');
  const [packetsRx, setPacketsRx] = useState(142);
  const [packetsTx, setPacketsTx] = useState(89);

  return (
    <div style={{ fontFamily: 'system-ui, -apple-system, sans-serif', padding: '24px', maxWidth: '800px', margin: '0 auto', color: '#1e293b' }}>
      {/* Header */}
      <header style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', borderBottom: '1px solid #e2e8f0', paddingBottom: '16px', marginBottom: '24px' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <Shield color="#2563eb" size={32} />
          <div>
            <h1 style={{ margin: 0, fontSize: '24px', fontWeight: 700 }}>Project Gibberish</h1>
            <p style={{ margin: 0, fontSize: '13px', color: '#64748b' }}>Zero-Trust Blind Encrypted Mesh Swarm & Clipboard Sync</p>
          </div>
        </div>
        <div style={{ display: 'flex', gap: '8px' }}>
          <span style={{ fontSize: '12px', padding: '4px 8px', borderRadius: '12px', backgroundColor: storageMode === 'RAM_ONLY' ? '#fef3c7' : '#dcfce7', color: storageMode === 'RAM_ONLY' ? '#92400e' : '#166534', fontWeight: 600 }}>
            SD: {storageMode}
          </span>
          <span style={{ fontSize: '12px', padding: '4px 8px', borderRadius: '12px', backgroundColor: '#e0e7ff', color: '#3730a3', fontWeight: 600 }}>
            802.15.4 Ch 15
          </span>
        </div>
      </header>

      {/* Navigation Tabs */}
      <nav style={{ display: 'flex', gap: '8px', marginBottom: '20px' }}>
        <button
          onClick={() => setActiveTab('clipboard')}
          style={{ padding: '8px 16px', borderRadius: '6px', border: 'none', cursor: 'pointer', backgroundColor: activeTab === 'clipboard' ? '#2563eb' : '#f1f5f9', color: activeTab === 'clipboard' ? '#fff' : '#475569', fontWeight: 600, display: 'flex', alignItems: 'center', gap: '8px' }}>
          <Clipboard size={16} /> Clipboard
        </button>
        <button
          onClick={() => setActiveTab('chat')}
          style={{ padding: '8px 16px', borderRadius: '6px', border: 'none', cursor: 'pointer', backgroundColor: activeTab === 'chat' ? '#2563eb' : '#f1f5f9', color: activeTab === 'chat' ? '#fff' : '#475569', fontWeight: 600, display: 'flex', alignItems: 'center', gap: '8px' }}>
          <Radio size={16} /> Mesh Chat
        </button>
        <button
          onClick={() => setActiveTab('vault')}
          style={{ padding: '8px 16px', borderRadius: '6px', border: 'none', cursor: 'pointer', backgroundColor: activeTab === 'vault' ? '#2563eb' : '#f1f5f9', color: activeTab === 'vault' ? '#fff' : '#475569', fontWeight: 600, display: 'flex', alignItems: 'center', gap: '8px' }}>
          <HardDrive size={16} /> Sneakernet Vault
        </button>
        <button
          onClick={() => setActiveTab('keyring')}
          style={{ padding: '8px 16px', borderRadius: '6px', border: 'none', cursor: 'pointer', backgroundColor: activeTab === 'keyring' ? '#2563eb' : '#f1f5f9', color: activeTab === 'keyring' ? '#fff' : '#475569', fontWeight: 600, display: 'flex', alignItems: 'center', gap: '8px' }}>
          <Key size={16} /> Swarm Keyring
        </button>
      </nav>

      {/* Tab Content */}
      <main style={{ backgroundColor: '#fff', border: '1px solid #e2e8f0', borderRadius: '8px', padding: '20px' }}>
        {activeTab === 'clipboard' && (
          <div>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
              <h2 style={{ fontSize: '18px', margin: 0 }}>Decentralized Clipboard Synchronization</h2>
              <label style={{ display: 'flex', alignItems: 'center', gap: '8px', fontSize: '14px', cursor: 'pointer' }}>
                <input
                  type="checkbox"
                  checked={autoSync}
                  onChange={(e) => setAutoSync(e.target.checked)}
                />
                Auto-Sync Clipboard
              </label>
            </div>
            <textarea
              value={clipboardText}
              onChange={(e) => setClipboardText(e.target.value)}
              rows={4}
              style={{ width: '100%', padding: '12px', boxSizing: 'border-box', borderRadius: '6px', border: '1px solid #cbd5e1', fontFamily: 'monospace', fontSize: '14px' }}
            />
            <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: '12px' }}>
              <button
                onClick={() => {
                  setPacketsTx(packetsTx + 1);
                  alert('Pushed encrypted 80B chunks across mesh!');
                }}
                style={{ padding: '8px 16px', backgroundColor: '#10b981', color: '#fff', border: 'none', borderRadius: '6px', fontWeight: 600, cursor: 'pointer' }}>
                Push to Swarm (Ctrl+Alt+C)
              </button>
              <span style={{ fontSize: '13px', color: '#64748b' }}>
                Type-Level Secret &lt;REDACTED&gt; Guardrails Active
              </span>
            </div>
          </div>
        )}

        {activeTab === 'chat' && (
          <div>
            <h2 style={{ fontSize: '18px', margin: 0, marginBottom: '12px' }}>1-on-1 Ratchet & Sender Keys Group Channels</h2>
            <div style={{ backgroundColor: '#f8fafc', padding: '12px', borderRadius: '6px', height: '200px', overflowY: 'auto', marginBottom: '12px' }}>
              <div style={{ marginBottom: '8px' }}>
                <strong style={{ color: '#2563eb' }}>Alice:</strong> Hey, syncing files over the 802.15.4 mesh!
              </div>
              <div style={{ marginBottom: '8px' }}>
                <strong style={{ color: '#059669' }}>Bob:</strong> Overhearing cancellation prevented duplicate RF broadcasts on our hop.
              </div>
            </div>
            <input
              type="text"
              placeholder="Type encrypted mesh message..."
              style={{ width: '80%', padding: '8px 12px', borderRadius: '6px', border: '1px solid #cbd5e1' }}
            />
            <button
              style={{ width: '18%', marginLeft: '2%', padding: '8px 12px', backgroundColor: '#2563eb', color: '#fff', border: 'none', borderRadius: '6px', fontWeight: 600 }}>
              Broadcast
            </button>
          </div>
        )}

        {activeTab === 'vault' && (
          <div>
            <h2 style={{ fontSize: '18px', margin: 0, marginBottom: '12px' }}>MicroSD FAT32 Sneakernet Vault (`GIBBERISH/VAULT/`)</h2>
            <p style={{ fontSize: '14px', color: '#64748b' }}>
              When a MicroSD card is mounted, large multi-megabyte encrypted files sync asynchronously via circular container CHUNKS.BIN.
            </p>
            <div style={{ border: '2px dashed #cbd5e1', padding: '32px', textAlign: 'center', borderRadius: '8px', color: '#64748b' }}>
              Drag & Drop Encrypted Sneakernet Payload Here
            </div>
          </div>
        )}

        {activeTab === 'keyring' && (
          <div>
            <h2 style={{ fontSize: '18px', margin: 0, marginBottom: '12px' }}>Device Linking & Paper Backup (R5, R6)</h2>
            <div style={{ display: 'flex', gap: '24px', alignItems: 'center' }}>
              <div style={{ border: '1px solid #e2e8f0', padding: '16px', borderRadius: '8px', textAlign: 'center' }}>
                <QrCode size={120} color="#334155" />
                <p style={{ fontSize: '12px', margin: '8px 0 0 0', color: '#64748b' }}>Scan with Gibberish Mobile</p>
              </div>
              <div>
                <h4 style={{ margin: '0 0 8px 0' }}>BIP-39 12-Word Paper Backup Seed</h4>
                <div style={{ backgroundColor: '#f1f5f9', padding: '12px', borderRadius: '6px', fontFamily: 'monospace', fontSize: '13px' }}>
                  shield orbit velvet copper winter beacon timber mirror tunnel fabric silver walnut
                </div>
              </div>
            </div>
          </div>
        )}
      </main>

      {/* Telemetry Footer */}
      <footer style={{ marginTop: '20px', display: 'flex', justifyContent: 'space-between', fontSize: '12px', color: '#64748b' }}>
        <div>Packets RX: {packetsRx} | TX: {packetsTx}</div>
        <div>Blind Dongle Zero-Trust: Zero keys or plaintext on hardware</div>
      </footer>
    </div>
  );
};
export default App;
