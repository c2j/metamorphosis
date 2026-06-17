select /*+ use_cplan*/ fund_name,
   area_name,
   accno,
   accname,
   balance,
   bank_name,
   asset_type,
   coin_code,
   coin_name,
   security_name,
   sys_flag,
   cnt_flag,
   vald_flag,
   operator_name,
   check_user_name,
   sys_acnt_id
   from (select fi.fund_name,
      (select t.area_name
         from par_sys_area t
        where fi.area_code = t.area_code
        and to_char(now(), 'yyyymmdd') between t.inure_begin_date and
              t.inure_end_date) as area_name,
      temp.ACCNO,
      temp.ACCNAME,
      CASE temp.SYS_FLAG
        when '1' then
         to_char((select t.balance
                   from DAT_CLR_ACNT_BALANCE t
                  where t.asset_acnt_id = temp.asset_acnt_id
                    AND t.data_date = to_char(sysdate - 1, 'yyyymmdd')))
        when '2' then
         '��ϵͳ���˻�'
        else
         ''
      END as balance, -- ���
      temp.bank_name, -- ����������
      (select t.kind_name
         from dic_all_kind t
        where t.operation_kind = 'asset_type'
          and t.kind_id = temp.asset_type) as asset_type, -- �ʲ�����
      temp.coin_code,
      (select t.coin_name
         from par_sys_coin t
        where t.coin_code = temp.coin_code) as coin_name,
      (SELECT b.market_name || '--' || a.main_stock_code || '--' ||
              a.stock_short_name
         FROM par_sys_securities a, par_sys_market b, par_sys_acnt_info t
        WHERE a.main_market_code = b.market_code
          AND a.security_id = t.security_id
          AND t.acnt_id = temp.sys_acnt_id
          AND to_char(now(), 'yyyymmdd') BETWEEN a.inure_begin_date AND
              a.inure_end_date) as security_name, -- ��ӦͶ��Ʒ����
      temp.sys_flag,
      temp.cnt_flag,
      temp.vald_flag,
      (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.operator = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') operator_name,
      (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.check_user = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') check_user_name,
      temp.sys_acnt_id,
      row_number() over(ORDER BY temp.sys_acnt_id) rn
    from (
SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
     t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
     t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
     t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
     t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
     t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
   FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
   WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
) temp
    left join par_fund_info fi
    on temp.fund_code = fi.fund_code
    left join (select t.area_name,t.area_code
         from par_sys_area t
         where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
              t.inure_end_date) sysarea
    on fi.area_code = sysarea.area_code
    WHERE temp.sub_src_type = '8'
    AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
    and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
    and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
    and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
    and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
    and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
    and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
    and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code))
    where rn BETWEEN to_number(p_i_qrybeginpos) AND to_number(p_i_qrybeginpos) + to_number(p_i_qrynum) - 1
    ORDER BY rn;
