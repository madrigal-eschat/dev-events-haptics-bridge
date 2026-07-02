# [1.1.0](https://github.com/madrigal-eschat/dev-events-haptics-bridge/compare/v1.0.0...v1.1.0) (2026-07-02)


### Bug Fixes

* bind HTTP backend synchronously, validate before startup, strengthen tests ([68042b5](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/68042b585a6ae1a41f3f1600f552d953675c8d45))
* **buttplug:** clarify device id errors ([f694097](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/f69409705e802406d9c13acdf8f93e9717ffc67e))
* **buttplug:** fail repeated teardown ([fa95327](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/fa95327ce5268b575a6ddb21181b0abda70c3d42))
* **buttplug:** make startup atomic ([2aea894](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/2aea8944836cb540e239b540e5536ebff99e2dce))
* **buttplug:** preserve lookup-scoped resolution ([57026b0](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/57026b01348a22a1d5553bd1547d6bf9735ac751))
* **buttplug:** surface shutdown timeouts ([f59be08](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/f59be08e4016211e226f2a04db0354d5e655df90))
* **config:** make HttpConfig::default() yield the documented bind default ([c6a7ac6](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/c6a7ac6d60dec68055c4c34c7ae988a9f2bcdb8a))
* harden buttplug lifecycle ([2f47836](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/2f478369639d09f7dd831ca1f04d0a27755667d5))
* harden rule dispatch ([c45da68](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/c45da68596d4c6c407450666226d22a26e370543))
* **main:** preserve backend on dispatch ([2ffa1a0](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/2ffa1a0980719e907c88c9232c7f1a3f8b6fad50))
* preserve backend-local device ids ([55d837c](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/55d837c70275356d03d9ca0686395b940e3d177a))
* preserve resolved device ids ([9590cd1](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/9590cd1b4e3632fea59c8e116d06ad149f5da013))


### Features

* add buttplug config ([699b70f](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/699b70f5863c2845918b0c569ae6fd5acb48d4f3))
* async buttplug dispatch ([b1dfb38](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/b1dfb3868aa541ae53e58f3720052b3f7e03d0d1))
* **backend:** add http backend serving the last 10 events at GET / ([c398c23](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/c398c239d6662fa418bdc29b3aa2ec3cbea402e3))
* **backend:** register http backend in is_known/create, thread Config through ([bc686c5](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/bc686c509893c4e71f12cf8cc8262f1d78a140b5))
* **buttplug:** add device parsing ([4e56703](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/4e56703b3615a61198406f757313bbf1c6184682))
* **config:** add optional http section for the http backend's bind address ([5d7745f](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/5d7745f7c098ad8443a7b26a4f2dc935d8990558))
* **main:** call backend startup/validate_device, teardown backends on ctrl-c ([c448dda](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/c448ddab5214bf7b7a609152bbbe230160e83aa8))
* **main:** resolve devices once per firing ([b5a0eab](https://github.com/madrigal-eschat/dev-events-haptics-bridge/commit/b5a0eab37d0f76d1625a026b75a0107e802e09d3))

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
