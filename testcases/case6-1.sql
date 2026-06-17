SELECT /*+use_cplan*/s.account_id,
             s.accname,
             s.account_date,
             s.in_amount,
             s.describe,
             s.recipacc,
             s.recipnam,
             s.account_seqno,
             s.accnt_seqno,
             s.interface_seq,
             s.match_status,
             s.statusname,
             s.respond_date
        FROM (SELECT t.account_id,
                     (SELECT s.accname
                        FROM v_par_asset_acnt_info s
                       WHERE s.asset_acnt_id = t.account_id
                         AND rownum = 1) accname,
                     t.account_date,
                     t.in_amount,
                     t.describe,
                     d.recipacc,
                     d.recipnam,
                     t.account_seqno,
                     t.trade_code,
                     t.accnt_seqno,
                     t.match_status,
                     t.interface_seq,
                     decode(t.match_status, '0', '未匹配', '1', '已匹配') statusname,
                     t.respond_date,
                     row_number() over(ORDER BY t.account_date DESC, t.account_seqno, t.account_id, t.interface_seq,accno, d.serialno,
                     d.busidate, d.timestmp, d.updtranf, d.revtranf, d.trxcode, d.drcrf, d.amount, d.detailf, d.currtype, d.subcode, d.euoflag) rownm
                FROM dat_clr_cash_dtl t, dat_trustee_acnt_detail d
               WHERE t.trade_code IN ('2008801001', '2008802001')
                 AND t.account_id = in_accnt_id
                 AND t.match_status = in_match_status
                 AND t.account_date BETWEEN nvl(in_accnt_date1, '19000101') AND
                     nvl(in_accnt_date2, '99991231')
                 AND (t.respond_date BETWEEN
                     nvl(in_respond_date1, '19000101') AND
                     nvl(in_respond_date2, '99991231') OR
                     t.respond_date IS NULL)
                 AND t.interface_seq = d.interface_seq(+)
                 AND (t.operation_status =
                     decode(t.trade_code,
                             '2008801001',
                             '0',
                             t.operation_status) OR
                     decode(t.trade_code,
                             '2008801001',
                             '0',
                             t.operation_status) IS NULL)) s
         limit to_number(in_qrynum) offset to_number(in_qrybeginpos)-1;
