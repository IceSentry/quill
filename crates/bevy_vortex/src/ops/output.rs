use bevy::prelude::*;

use crate::{
    gen::Expr,
    operator::{
        DisplayName, Operator, OperatorCategory, OperatorClass, OperatorDescription, OperatorInput,
        OperatorInputOnly, ReflectOperator,
    },
};

#[derive(Debug, Reflect, Clone, Default)]
#[reflect(Operator, Default, @OperatorClass(OperatorCategory::Output), @OperatorDescription("
Displays the output of the shader.
"))]
pub struct Output {
    #[reflect(@OperatorInput, @OperatorInputOnly, @DisplayName("Color"))]
    pub input: LinearRgba,
}

impl Operator for Output {
    fn to_boxed_clone(&self) -> Box<dyn Operator> {
        Box::new(self.clone())
    }

    fn gen(&self) -> Expr {
        // todo!()
        Expr::ConstColor(LinearRgba::WHITE)
    }
}
