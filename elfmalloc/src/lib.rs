// Copyright 2017 the authors. See the 'Copyright and license' section of the
// README.md file at the top-level directory of this repository.
//
// Licensed under the Apache License, Version 2.0 (the LICENSE file). This file
// may not be copied, modified, or distributed except according to those terms.

#![feature(alloc)]
#![feature(allocator_api)]
#![cfg_attr(feature = "nightly", feature(thread_local_state))]
#![cfg_attr(feature = "nightly", feature(thread_local))]
#![cfg_attr(feature = "nightly", feature(const_fn))]
#![cfg_attr(feature = "nightly", feature(cfg_target_thread_local))]
#![cfg_attr(feature = "nightly", feature(core_intrinsics))]
extern crate alloc;
extern crate bagpipe;

// Linking in `bsalloc` causes it to be used as the global heap allocator. That is important when
// using this as a basis for a `malloc` library, but it becomes a hindrance when using this crate
// as a specialized allocator library.
#[cfg(not(feature = "use_default_allocator"))]
extern crate bsalloc;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;

mod utils;
#[macro_use]
mod stats;
pub mod slag;
pub mod general;
