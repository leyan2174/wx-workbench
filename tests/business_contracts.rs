//! Compile and exercise real business modules without their concrete adapters.
//! Tests inside those modules use only in-memory sources.
#![allow(dead_code)]

#[path = "../src/business/mod.rs"]
mod business;
