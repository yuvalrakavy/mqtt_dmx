
use std::collections::HashMap;

use super::manager::ArrayManager;
use super::error::DmxArrayError;
use super::Scope;


impl ArrayManager {
    pub (super) fn add_values(&mut self, values: HashMap<String, String>) -> Result<(), DmxArrayError> {
        self.values = values;
        Ok(())
    }

    pub (super) fn remove_values(&mut self) -> Result<(), DmxArrayError>{
        self.values.clear();
        Ok(())
    }

    fn get_value(&self, scope: &Scope, value_name: &str) -> Result<Option<String>, DmxArrayError> {
        if let Some(values) = &scope.values {
            if let Some(value) = values.get(value_name) {
                return Ok(Some(value.to_string()));
            }
        }

        let array = self.get_array(&scope.array_id)?;

        if let Some(preset_number) = scope.preset_number {
            if let Some(preset) = array.presets.get(preset_number) {
                if let Some(value) = preset.values.get(value_name) {
                    return Ok(Some(value.to_string()));
                }
            }
            else {
                return Err(DmxArrayError::ArrayPresetNotFound(scope.array_id.clone(), preset_number));
            }
        }

        if let Some(value) = array.values.get(value_name) {
            return Ok(Some(value.to_string()));
        }

        Ok(self.values.get(value_name).map(|s| s.to_string()))
    }

    pub (super) fn expand_values(&self, scope: &Scope, unexpanded_value: &str) -> Result<String, DmxArrayError> {
        let mut value = unexpanded_value;
        let mut result = String::new();
        let index = 0;

        while let Some(value_name_start_index) = value[index..].find('`') {
            result.push_str(&value[..value_name_start_index]);
            value = &value[value_name_start_index + 1..];

            if let Some(value_name_end_index) = value.find('`') {
                let value_name_expression = &value[..value_name_end_index];
                let (value_name, default_value) = if let Some(default_value_index) = value_name_expression.find('=') {
                    (&value_name_expression[..default_value_index], Some(&value_name_expression[default_value_index + 1..]))
                }
                else {
                    (value_name_expression, None)
                };

                let expanded_value = self.get_value(scope, value_name)?;

                if let Some(expanded_value) = expanded_value {
                    result.push_str(&expanded_value);
                }
                else if let Some(default_value) = default_value {
                    result.push_str(default_value);
                }
                else {
                    return Err(
                        if let Some(preset_number) = scope.preset_number {
                            DmxArrayError::ArrayPresetValueNotFound(scope.array_id.to_string(), preset_number, unexpanded_value.to_string(), value_name.to_string())
                        }
                        else {
                            DmxArrayError::ArrayValueNotFound(scope.array_id.to_string(), unexpanded_value.to_string(), value_name.to_string())
                        }
                    );
                }

                value = &value[value_name_end_index + 1..];
            }
            else {
                return Err(DmxArrayError::ValueExpressionNotTerminated(scope.array_id.clone(), unexpanded_value.to_string()));
            }
        }

        result.push_str(value);

        Ok(result)
    }

}

impl crate::defs::NumberOrVariable {
    pub fn get_value(&self, scope: &Scope) -> Result<usize, DmxArrayError> {
        match self {
            crate::defs::NumberOrVariable::Number(n) => Ok(*n),
            crate::defs::NumberOrVariable::Variable(s) => {
                let value = scope.expand_values(s)?;
                value.parse().map_err(|e: std::num::ParseIntError| DmxArrayError::ValueNotANumber(scope.to_string(),"ticks parameter",  e.to_string()))
            }
        }
    }
}