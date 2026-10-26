-- 12-inch MacBook (CS4208): the speakers have no hardware volume, so PipeWire does it in software.
-- From https://github.com/leifliddy/macbook12-audio-driver (WirePlumber 0.4, as on Ubuntu 24.04).
table.insert(alsa_monitor.rules, {
  matches = {
    { { "device.name", "equals", "alsa_card.pci-0000_00_1f.3" } },
  },
  apply_properties = {
    ["api.alsa.soft-mixer"] = true,
  },
})
