-- $ cat line-*.sql
SELECT count(0)
FROM (
(SELECT bs.inst_num
, (
SELECT t.kind_name
FROM ebk_dic_all_kind t
WHERE (t.operation_kind = CASE
WHEN lower(?) = ? THEN ?
ELSE ?
END
OR t.operation_kind = CASE
WHEN lower(?) = ? THEN ?
ELSE ?
END
OR t.operation_kind = CASE
WHEN lower(?) = ? THEN ?
ELSE ?
END)
AND t.kind_id = bs.operation_no
AND rownum = ?
) AS class_name, nvl2(bs.plan_id, (
SELECT u.plan_name
FROM par_sys_plan u
WHERE u.plan_id = bs.plan_id
AND bs.inst_data_date BETWEEN u.inure_begin_date AND u.inure_end_date
), (
SELECT CASE
WHEN LOWER(?) = ? THEN m.fund_name_en
ELSE m.fund_name
END
FROM par_fund_info m
WHERE m.fund_code = bs.fund_code
)) AS fund_name
, bs.cap_date, bs.sec_date, tr.cash
, (
SELECT f.kind_name
FROM dic_all_kind f
WHERE f.operation_kind = ?
AND f.kind_id = st.bank_sys_status
) AS bank_sys_status_name
, CASE
WHEN LOWER(?) = ? THEN (
SELECT k.kind_name
FROM ebk_dic_all_kind k
WHERE k.operation_kind = ?
AND k.kind_id = st.tran_status
)
ELSE (
SELECT g.kind_name
FROM dic_all_kind g
WHERE g.operation_kind = ?
AND g.kind_id = st.tran_status
)
END AS tran_status_name
, (
SELECT n.stock_code
FROM v_ebk_dat_inst_secu_deal_info n
WHERE bs.inst_num = n.parent_seq_no
AND bs.inst_data_date = n.inst_data_date
AND rownum = ?
) AS csbsStatusName, st.inst_status AS inst_status
, pkg_ebank_fund_order.get_handle_user_or_mobile(st.inst_status, st.current_step, bs.fund_code, bs.user_code, ?) AS user_name
, (
SELECT CASE
WHEN LOWER(?) = ? THEN n.stock_en_sname
ELSE n.stock_short_name
END AS stock_name
FROM v_ebk_dat_inst_secu_deal_info n
WHERE bs.inst_num = n.parent_seq_no
AND bs.inst_data_date = n.inst_data_date
AND rownum = ?
) AS stock_name, fnc_ebank_com_get_account_code(bs.operation_no, bs.inst_data_date, tr.out_dept_acnt) AS out_dept_acnt
, fnc_com_get_account_name(bs.operation_no, bs.inst_data_date, tr.out_dept_acnt) AS out_dept_acnt_name
, fnc_ebank_com_get_account_code(bs.operation_no, bs.inst_data_date, decode(bs.settle_mode, ?, decode(tr.in_out, ?, tr.in_dvp_acnt, tr.in_dept_acnt), tr.in_dept_acnt)) AS in_dept_acnt
, fnc_com_get_account_name(bs.operation_no, bs.inst_data_date, decode(bs.settle_mode, ?, decode(tr.in_out, ?, tr.in_dvp_acnt, tr.in_dept_acnt), tr.in_dept_acnt)) AS in_dept_acnt_name
, tr.remark
, pkg_ebank_fund_order.get_handle_user_or_mobile(st.inst_status, st.current_step, bs.fund_code, bs.user_code, ?) AS user_phone
, bs.data_source
, (
SELECT dc.kind_name
FROM dic_all_kind dc
WHERE dc.operation_kind = ?
AND dc.kind_id = bs.data_source
) AS data_source_name, bs.inst_data_date, st.current_step_name
FROM v_dat_inst_base_info_s bs, v_dat_inst_tran_info tr, v_dat_inst_status_info st
WHERE bs.inst_data_date = st.inst_data_date
AND bs.inst_data_date = tr.inst_data_date(+)
AND bs.inst_num = st.inst_num
AND bs.inst_num = tr.inst_num(+)
AND ((bs.is_plan = ?
AND bs.fund_code IN (
SELECT fund_code
FROM par_netuser_fund
WHERE valid_flag = ?
AND user_id = ?
))
OR (bs.is_plan = ?
AND bs.plan_id IN (
SELECT DISTINCT a.plan_id
FROM par_sys_plan a, par_sys_organ b, dat_clnt_cstdy_info c, exterior_user_info d, v_par_asset_acnt_info v
WHERE a.organ_code = b.organ_code
AND b.type_code = ?
AND b.v_dept_code = c.dept_code
AND c.cis_code = d.cis
AND d.user_id = ?
AND a.acnt_id = v.asset_acnt_id
AND v.acnt_type = ?
AND TO_CHAR(SYSDATE, ?) BETWEEN a.inure_begin_date AND a.inure_end_date
AND TO_CHAR(SYSDATE, ?) BETWEEN b.inure_begin_date AND b.inure_end_date
AND TO_CHAR(SYSDATE, ?) BETWEEN v.inure_begin_date AND v.inure_end_date
AND a.acnt_id IN (
SELECT t.acnt_code
FROM par_netuser_fund t
WHERE t.valid_flag = ?
AND t.acnt_type = ?
AND t.user_id = ?
)
)))
AND EXISTS (
SELECT op.kind_id
FROM ebk_dic_all_kind op
WHERE (op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END)
AND op.kind_id = bs.operation_no
AND op.kind_id = ?
)
AND bs.cap_date >= ?
AND bs.cap_date <= ?
AND st.inst_status <> ?
AND bs.fund_code = ?)
UNION ALL
(SELECT bs.inst_num
, (
SELECT t.kind_name
FROM ebk_dic_all_kind t
WHERE (t.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR t.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR t.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END)
AND t.kind_id = bs.operation_no
AND rownum = ?
) AS class_name, nvl2(bs.plan_id, bs.plan_name, (
SELECT CASE
WHEN LOWER(?) = ? THEN m.fund_name_en
ELSE m.fund_name
END
FROM par_fund_info m
WHERE m.fund_code = bs.fund_code
)) AS fund_name
, bs.cap_date, bs.sec_date, tr.cash, st.bank_sys_status_name
, CASE
WHEN LOWER(?) = ? THEN (
SELECT k.kind_name
FROM ebk_dic_all_kind k
WHERE k.operation_kind = ?
AND k.kind_id = st.tran_status
)
ELSE st.tran_status_name
END AS tran_status_name
, (
SELECT n.stock_code
FROM v_ebk_dat_inst_secu_deal_info n
WHERE bs.inst_num = n.parent_seq_no
AND bs.inst_data_date = n.inst_data_date
AND rownum = ?
) AS stock_code, st.inst_status AS inst_status
, pkg_ebank_fund_order.get_handle_user_or_mobile(st.inst_status, st.current_step, bs.fund_code, bs.user_code, ?) AS user_name
, (
SELECT CASE
WHEN LOWER(?) = ? THEN n.stock_en_sname
ELSE n.stock_short_name
END AS stock_name
FROM v_ebk_dat_inst_secu_deal_info n
WHERE bs.inst_num = n.parent_seq_no
AND bs.inst_data_date = n.inst_data_date
AND rownum = ?
) AS stock_name, fnc_ebank_com_get_account_code(bs.operation_no, bs.inst_data_date, tr.out_dept_acnt) AS out_dept_acnt
, fnc_com_get_account_name(bs.operation_no, bs.inst_data_date, tr.out_dept_acnt) AS out_dept_acnt_name
, fnc_ebank_com_get_account_code(bs.operation_no, bs.inst_data_date, decode(bs.settle_mode, ?, decode(tr.in_out, ?, tr.in_dvp_acnt, tr.in_dept_acnt), tr.in_dept_acnt)) AS in_dept_acnt
, fnc_com_get_account_name(bs.operation_no, bs.inst_data_date, decode(bs.settle_mode, ?, decode(tr.in_out, ?, tr.in_dvp_acnt, tr.in_dept_acnt), tr.in_dept_acnt)) AS in_dept_acnt_name
, tr.remark
, pkg_ebank_fund_order.get_handle_user_or_mobile(st.inst_status, st.current_step, bs.fund_code, bs.user_code, ?) AS user_phone
, bs.data_source
, (
SELECT dc.kind_name
FROM dic_all_kind dc
WHERE dc.operation_kind = ?
AND dc.kind_id = bs.data_source
) AS data_source_name, bs.inst_data_date, st.current_step_name
FROM (
(SELECT bs.*, ? AS plan_id, ? AS plan_name, ? AS is_plan
FROM dat_inst_base_info_mon bs)
) bs, dat_inst_tran_info_mon tr, v_dat_inst_status_info st
WHERE bs.inst_data_date = st.inst_data_date
AND bs.inst_data_date = tr.inst_data_date(+)
AND bs.inst_num = st.inst_num
AND bs.inst_num = tr.inst_num(+)
AND ((bs.is_plan = ?
AND bs.fund_code IN (
SELECT fund_code
FROM par_netuser_fund
WHERE valid_flag = ?
AND user_id = ?
))
OR (bs.is_plan = ?
AND bs.plan_id IN (
SELECT DISTINCT a.plan_id
FROM par_sys_plan a, par_sys_organ b, dat_clnt_cstdy_info c, exterior_user_info d, v_par_asset_acnt_info v
WHERE a.organ_code = b.organ_code
AND b.type_code = ?
AND b.v_dept_code = c.dept_code
AND c.cis_code = d.cis
AND d.user_id = ?
AND a.acnt_id = v.asset_acnt_id
AND v.acnt_type = ?
AND TO_CHAR(SYSDATE, ?) BETWEEN a.inure_begin_date AND a.inure_end_date
AND TO_CHAR(SYSDATE, ?) BETWEEN b.inure_begin_date AND b.inure_end_date
AND TO_CHAR(SYSDATE, ?) BETWEEN v.inure_begin_date AND v.inure_end_date
AND a.acnt_id IN (
SELECT t.acnt_code
FROM par_netuser_fund t
WHERE t.valid_flag = ?
AND t.acnt_type = ?
AND t.user_id = ?
)
)))
AND EXISTS (
SELECT op.kind_id
FROM ebk_dic_all_kind op
WHERE (op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END
OR op.operation_kind = CASE
WHEN LOWER(?) = ? THEN ?
ELSE ?
END)
AND op.kind_id = bs.operation_no
AND op.kind_id = ?
)
AND bs.cap_date >= ?
AND bs.cap_date <= ?
AND st.inst_status <> ?
AND bs.fund_code = ?
AND 1 = 0)
) t;
SELECT account_date,account_flag,check_type,check_type_shadow,coin_code,confirm_amount,confirm_quantity,deal_amount,deal_date,deal_quantity,fund_code,in_dept_acnt,inst_data_date,inst_num,jz_quantity,main_fee,market_code,operation_no,operator,security_id FROM bigfund.v_acva_inst_fund_acct_invest WHERE (fund_code = ? ) AND ((deal_date = ?  or account_date = ? ));
SELECT b.bank_name AS out_bank_name, b.accname AS out_acnt_name, ? AS out_dept_name, b.accno AS outdeptnoName
FROM v_all_acnt_info_base b
WHERE b.acnt_id = ?;
SELECT t.account_date
       FROM v_dat_cash_balance t
       WHERE t.account_id =  ?
       AND t.account_date = ?
       AND t.coin_code = ?;
