# streaming-data-parsing-with-actix-lua

An example of using Rust, actix, and Lua to parse streaming data with [actix-lua](https://github.com/poga/actix-lua).

[Blog Post](https://devpoga.org/post/parsing-streaming-data-actix-lua/<Paste>)

## License

The MIT License

## Note

This example does not implement back-pressure, hence not suitable for production usage (see [comment](https://www.reddit.com/r/rust/comments/9nijmg/analyze_streaming_data_with_rust_and_lua/e7qvrj2/)).

If you want to help adding back-pressure, send a PR. :)
