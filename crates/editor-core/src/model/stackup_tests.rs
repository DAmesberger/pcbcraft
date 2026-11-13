use super::*;

#[test]
fn every_even_count_up_to_the_bound_has_a_symmetric_preset_that_passes_the_rules() -> Result<()> {
    let counts: Vec<usize> = Stackup::preset_counts().collect();
    assert_eq!(counts.first(), Some(&MIN_COPPER_LAYERS));
    assert_eq!(counts.last(), Some(&MAX_COPPER_LAYERS));
    assert_eq!(counts.len(), 32);
    for count in counts {
        let s = Stackup::preset(count)?;
        s.check()?;
        assert_eq!(s.copper.len(), count);
        assert_eq!(s.layers().len(), count);
        assert_eq!(s.dielectrics.len(), count - 1);
        // Symmetric about the middle.
        let flipped: Vec<_> = s.dielectrics.iter().rev().copied().collect();
        assert_eq!(s.dielectrics, flipped, "{count} layers");
        if count > 2 {
            // Foil build: prepreg outside.
            assert_eq!(
                s.dielectrics.first().map(|d| d.kind),
                Some(DielectricKind::Prepreg)
            );
        }
        let dielectric = s.dielectric_mm();
        if count <= 10 {
            assert!((dielectric - 1.6).abs() < 0.01, "{count}: {dielectric}");
        } else {
            assert!(dielectric > 1.6, "{count}: {dielectric}");
        }
    }
    // The common presets keep their builds.
    assert_eq!(Stackup::preset(2)?, Stackup::default());
    let four = Stackup::preset(4)?;
    let thick: Vec<f64> = four.dielectrics.iter().map(|d| d.thickness_mm).collect();
    assert_eq!(thick, vec![0.2, 1.2, 0.2]);
    Ok(())
}

#[test]
fn the_rules_bound_the_count_require_pairs_and_one_dielectric_between_foils() -> Result<()> {
    for count in [0, 1, 3, 5, 63, 65, 66, 1000] {
        assert!(Stackup::preset(count).is_err(), "{count}");
    }
    let mut odd = Stackup::preset(6)?;
    odd.copper.pop();
    odd.dielectrics.pop();
    let refused = odd.check();
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.contains("even")),
        "{refused:?}"
    );
    let mut big = Stackup::preset(MAX_COPPER_LAYERS)?;
    big.check()?;
    let dielectric = big.dielectrics[0];
    big.copper.extend([CopperFoil::of_um(35.0); 2]);
    big.dielectrics.extend([dielectric; 2]);
    assert!(matches!(big.check(), Err(Error::Invalid(m)) if m.contains("at most 64")));
    let mut gap = Stackup::preset(8)?;
    gap.dielectrics.pop();
    assert!(matches!(gap.check(), Err(Error::Invalid(m)) if m.contains("one dielectric")));
    let mut thin = Stackup::preset(8)?;
    thin.dielectrics[3].thickness_mm = 0.01;
    assert!(thin.check().is_err());
    let mut dk = Stackup::preset(8)?;
    dk.dielectrics[3].dk = Some(f64::NAN);
    assert!(dk.check().is_err());
    let mut heavy = Stackup::preset(8)?;
    heavy.copper[5].thickness_um = 400.0;
    assert!(heavy.check().is_err());
    Ok(())
}

#[test]
fn a_many_layer_stackup_names_and_places_every_layer() -> Result<()> {
    let s = Stackup::preset(MAX_COPPER_LAYERS)?;
    let layers = s.layers();
    assert_eq!(layers[0], CopperLayer::Top);
    assert_eq!(layers[62], CopperLayer::Inner(62));
    assert_eq!(layers[63], CopperLayer::Bottom);
    assert!(s.has(CopperLayer::Inner(62)));
    assert!(!s.has(CopperLayer::Inner(63)));
    // Each layer lies below the one above it.
    let mut above = f64::INFINITY;
    for layer in layers {
        let (low, high) = s
            .copper_span(layer)
            .ok_or_else(|| Error::Invalid(format!("no span for {layer:?}")))?;
        assert!(high <= above + 1e-9 && low < high, "{layer:?}");
        above = low;
    }
    Ok(())
}
