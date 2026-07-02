# 1.0.0 (2026-06-30)


* feat(config)!: support multi-device rules via DeviceSpec ([d3aef59](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/d3aef590812a9d9b4d50420a6c445b6d22e80733))


### Bug Fixes

* **ci:** correct repo name in cross-repo workflow references ([b2619e8](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/b2619e8623d06140f252d64c6a675b38a3cd99d5))
* **ci:** grant required permissions for cross-repo semrel call ([4429f06](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/4429f0652944527a6446b90c6c06403e992bec1e))
* **hooks:** block commits on fmt/check/test failures, fix fmt violations ([a8c06be](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/a8c06beaa4c76e5d6c5dfaf264e783c0d38e5ffe))


### Features

* **gestures:** add stop and stop_all silence gestures ([bbbaca2](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/bbbaca2434eba083795660b65c0e511a612a59ec))


### BREAKING CHANGES

* multi-device gestures (both_*, crossfade_*, stop_all)
now require `devices: [...]` in the rule instead of a single `device:`
