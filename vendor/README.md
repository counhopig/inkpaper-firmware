# Vendored Rust dependencies

`esp-idf-hal` 0.46.2 is vendored to add the `sdmmc_host_t` field introduced by
ESP-IDF 5.5.5. Remove this patch after a released `esp-idf-hal` version supports
that ESP-IDF patch release.
