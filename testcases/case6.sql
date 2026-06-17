    SELECT /*+use_cplan*/COUNT(1)
      INTO totalnum
      FROM dat_clr_cash_dtl t, dat_trustee_acnt_detail d
     WHERE t.trade_code IN ('2008801001', '2008802001')
       AND t.account_id = in_accnt_id
       AND t.match_status = in_match_status
       AND t.account_date BETWEEN nvl(in_accnt_date1, '19000101') AND
           nvl(in_accnt_date2, '99991231')
       AND (t.respond_date BETWEEN nvl(in_respond_date1, '19000101') AND
           nvl(in_respond_date2, '99991231') OR t.respond_date IS NULL)
       AND t.interface_seq = d.interface_seq(+)
       AND (t.operation_status =
           decode(t.trade_code, '2008801001', '0', t.operation_status) OR
           decode(t.trade_code, '2008801001', '0', t.operation_status) IS NULL);
