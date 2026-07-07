// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

#[path = "common/prime.rs"]
pub mod prime;
#[path = "common/profile_proto.rs"]
pub mod profile_proto;
#[path = "common/threaded.rs"]
pub mod threaded;

fn main() -> profile_proto::ExampleResult<()> {
  threaded::run_post_processed_reports()
}
