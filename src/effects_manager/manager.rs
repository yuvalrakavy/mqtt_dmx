

pub trait EffectNodeRuntime {
    fn tick(&mut self);
    fn is_done(&self) -> bool;
}
