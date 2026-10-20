//! Prints the service's OpenAPI document as the repository keeps it in `docs/openapi.json`.

fn main() {
  print!("{}", bmo_cache_warmer::openapi::openapi_json());
}
