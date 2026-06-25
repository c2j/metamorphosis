select close_date
    into v_close_date
    from (select t.close_date, t.market_code, p_i_coincode coin_code
        from par_oper_close_date t
        where t.market_code = p_i_scdm
        and (t.coin_code = '000' or t.coin_code = p_i_coincode))
    where market_code = p_i_scdm
    and coin_code = p_i_coincode
    and close_date = to_char(v_date, 'yyyymmdd')
