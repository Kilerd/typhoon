use lalrpop_util::lalrpop_mod;

pub(crate) mod handler;
lalrpop_mod!(#[allow(clippy::all)]pub parser);
