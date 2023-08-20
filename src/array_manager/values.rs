use std::sync::Arc;
use crate::defs::SymbolTable;

use super::error::DmxArrayError;
use super::manager::ArrayManager;
use super::Scope;

impl ArrayManager {
    fn set_array_value(
        &mut self,
        array_id: Arc<str>,
        value_name: Arc<str>,
        value: &str,
    ) -> Result<(), DmxArrayError> {
        let array_values = self
            .values
            .entry(array_id)
            .or_insert_with(SymbolTable::new);

        array_values.insert(value_name, value.to_string());
        Ok(())
    }

    pub (super) fn initialize_array_values(
        &mut self,
        array_id: Arc<str>,
        symbol_table: SymbolTable,
    ) -> Result<(), DmxArrayError> {
        for (value_name, value) in symbol_table {
            self.set_array_value(array_id.clone(), value_name, &value)?;
        }
        Ok(())
    }

    pub(super) fn set_global_value(&mut self, value_name: Arc<str>, value: &str) -> Result<(), DmxArrayError> {
        self.global_values.insert(value_name, value.to_string());
        Ok(())
    }

    pub(super) fn remove_global_value(
        &mut self,
        value_name: &str,
    ) -> Result<(), DmxArrayError> {
        self.global_values.remove(value_name);
        Ok(())
    }

    fn get_value(
        &self,
        array_id: Arc<str>,
        value_name: &str,
    ) -> Result<Option<String>, DmxArrayError> {
        if !self.arrays.contains_key(&array_id) {
            return Err(DmxArrayError::ArrayNotFound(array_id));
        }

        if let Some(array_values) = self.values.get(&array_id) {
            if let Some(value) = array_values.get(value_name) {
                return Ok(Some(value.to_string()));
            }
        }

        Ok(self.global_values.get(value_name).map(|s| s.to_string()))
    }

    pub(super) fn expand_values(
        &self,
        array_id: Arc<str>,
        unexpanded_value: &str,
    ) -> Result<String, DmxArrayError> {
        let mut value = unexpanded_value;
        let mut result = String::new();
        let index = 0;

        while let Some(value_name_start_index) = value[index..].find('`') {
            result.push_str(&value[..value_name_start_index]);
            value = &value[value_name_start_index + 1..];

            if let Some(value_name_end_index) = value.find('`') {
                let value_name_expression = &value[..value_name_end_index];
                let (value_name, default_value) =
                    if let Some(default_value_index) = value_name_expression.find('=') {
                        (
                            &value_name_expression[..default_value_index],
                            Some(&value_name_expression[default_value_index + 1..]),
                        )
                    } else {
                        (value_name_expression, None)
                    };

                let expanded_value = self.get_value(array_id.clone(), value_name)?;

                if let Some(expanded_value) = expanded_value {
                    result.push_str(&expanded_value);
                } else if let Some(default_value) = default_value {
                    result.push_str(default_value);
                } else {
                    return Err(DmxArrayError::ArrayValueNotFound(
                        array_id.clone(),
                        unexpanded_value.to_string(),
                        value_name.to_string(),
                    ));
                }

                value = &value[value_name_end_index + 1..];
            } else {
                return Err(DmxArrayError::ValueExpressionNotTerminated(
                    array_id.clone(),
                    Arc::from(unexpanded_value),
                ));
            }
        }

        result.push_str(value);

        Ok(result)
    }
}

impl crate::defs::NumberOrVariable {
    pub fn get_value(
        &self,
        scope: &Scope,
        description: &'static str,
    ) -> Result<usize, DmxArrayError> {
        match self {
            crate::defs::NumberOrVariable::Number(n) => Ok(*n),
            crate::defs::NumberOrVariable::Variable(s) => {
                let value = scope.expand_values(s)?;
                value.parse().map_err(|e: std::num::ParseIntError| {
                    DmxArrayError::ValueError(scope.to_string(), description, e.to_string())
                })
            }
        }
    }
}
