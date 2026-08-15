# Vendored Rust dependencies

本仓库对上游 Rust 包做了本地化改动时，源码直接放在 `vendor/` 下，通过 Cargo 的 path 依赖引用。

## esp-idf-hal 0.46.2

`vendor/esp-idf-hal/` 是上游 `esp-idf-hal v0.46.2` 的本地副本，增加了 ESP-IDF 5.5.5 引入的 `sdmmc_host_t` 字段。补丁详情仅在原仓库的 fork/commit 中可见；本地副本与上游保持同步时不要丢弃这些新增字段。

Cargo 中以 path 方式引用：

```toml
[patch.crates-io]
esp-idf-hal = { path = "../vendor/esp-idf-hal" }
```

当 crates.io 上发布的 `esp-idf-hal` 提供支持 ESP-IDF 5.5.5 的版本后，删除本目录并删除 `Cargo.toml` 的 `[patch.crates-io]` 段，恢复普通 `esp-idf-hal` 依赖。

## zectrix_epd C++ 组件

NOTE4 EPD C++ 驱动（`SSD2683` + 校准波形表）同样以本地 component 形式被嵌入：

```
rust-firmware/components/zectrix_epd/
```

通过 `package.metadata.esp-idf-sys.extra_components` 直接在 Rust 编译时用 bindgen 生成 `zectrix_epd` 模块的 FFI。
