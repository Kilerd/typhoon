use crate::context::TyphoonContext;
use std::sync::Arc;

trait Codegen {
    fn codegen(&self, context: Arc<TyphoonContext>);
}
